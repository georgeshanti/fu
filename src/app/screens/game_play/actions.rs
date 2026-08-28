use std::{collections::{BTreeMap, BTreeSet}, f32::consts::PI, time::SystemTime};

use avian3d::prelude::*;
use bevy::{ecs::system::SystemState, prelude::*, transform};

use crate::{app::{GameClientWrapper, screens::{app_state::AppState, game_play::{animations::{Dying, Swinging, ThrowingAnimation}, entities::boomerang::{self, Boomerang, Thrown}, phases::end::Score, state::{Dead, InReplay, LocalGameEvents, PlayerId, Ticker, record_player_action}, world::{GameLayer, spawn_world}}}}, server::{ClientEvent, Controller, OrderedF32, PlayerAction, ServerEvent}};

/// Constant linear acceleration applied to the player to overcome ground friction.
/// Derived from Coulomb friction: μ × g = 0.5 × 9.81 = 4.905 m/s²
const PLAYER_ACCEL: f32 = 4.905 * 2.0;

/// Minimum delay after a swing ends before the player may swing again.
const JUMP_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(900);

#[derive(Resource)]
pub struct PlayerJumpCooldowns(pub BTreeMap<u8, std::time::SystemTime>);

#[derive(Component)]
pub struct StartThrow;

/// Minimum delay after a swing ends before the player may swing again.
const SWING_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(800);

#[derive(Resource)]
pub struct PlayerSwingCooldowns(pub BTreeMap<u8, std::time::SystemTime>);

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Minimum time between movement events when direction is unchanged (keep-alive heartbeat), in seconds.
const DIRECTION_EVENT_INTERVAL: u128 = 500;

/// Quantization factor for joystick axes: 2^7 / 2 = 64 levels per side,
/// giving 128 discrete steps across the clamped -1..1 range.
const DIRECTION_QUANTIZATION: f32 = 4.0;

#[derive(Clone)]
pub struct PlayerDirection {
    time: std::time::SystemTime,
    direction_facing: Vec3,
    velocity: Vec3,
}

#[derive(Resource, Default, Clone)]
pub struct PlayerDirections(pub BTreeMap<u8, PlayerDirection>);

#[derive(Resource, Default, Clone)]
pub struct PlayersPulling(pub BTreeSet<u8>);

#[derive(Resource)]
pub struct PlayerActions {
    pub actions: Vec<PlayerAction>,
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
        Query<(Entity, &PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform, Option<&Children>), (With<PlayerId>, Without<Dying>, Without<Boomerang>)>,
        Query<(Entity, &Transform), (With<Boomerang>, Without<Swinging>)>,
        ResMut<Ticker>,
        ResMut<LocalGameEvents>,
        Query<Entity, With<PlayerId>>,
        Query<Entity, (With<Boomerang>, With<Thrown>)>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<InReplay>,
        ResMut<NextState<AppState>>,
        ResMut<PlayerActions>,
    )>>,
) {
    let (mut new_player_actions, server_events) = {
        let client = params.get_mut(world).1;
        let client = client.client.read().unwrap();
        let events = {
            let mut server_events = client.received_events.lock().unwrap();
            let events = server_events.clone();
            *server_events = vec![];
            events
        };
        let mut player_actions: Vec<(u64, PlayerAction)> = events.iter().filter_map(|event| { if let ServerEvent::PlayerAction{tick: tick, game_event: game_event} = event { Some((*tick, game_event.clone())) } else { None } }).collect();
        player_actions.sort_by(|a, b| {a.0.cmp(&b.0)});
        (player_actions, events)
    };
    for server_event in server_events {
        match server_event {
            ServerEvent::RoundEnded{ max, old_score, new_score} => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                let (mut commands, _, _, _, _, _, _, _, _, _, _, mut next_state, _) = params.get_mut(world);
                commands.insert_resource(Score{time: SystemTime::now(), max, old_score, new_score, winners: vec![]});
                next_state.set(AppState::RoundEndAnimation);
                params.apply(world);
                return;
            },
            ServerEvent::GameEnded{ max, old_score, new_score, game_winners} => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                let (mut commands, _, _, _, _, _, _, _, _, _, _, mut next_state, _) = params.get_mut(world);
                commands.insert_resource(Score{time: SystemTime::now(), max, old_score, new_score, winners: game_winners.iter().map(|winner| {winner.id}).collect()});
                next_state.set(AppState::RoundEndAnimation);
                params.apply(world);
                return;
            },
            _ => {}
        }
    }
    if !new_player_actions.is_empty() {
        let final_tick = {
            let ticker = params.get_mut(world).4;
            std::cmp::max(new_player_actions.last().unwrap().0, ticker.0)
        };
        let mut existing_records = {
            let first_tick = new_player_actions.first().unwrap().0;
            let (mut commands, _, _, _, mut ticker, mut local_game_events, players, boomerangs, mut meshes, mut materials, mut in_replay, _, _) = params.get_mut(world);
            let game_state = local_game_events.game_events.get(first_tick as usize).unwrap().game_state.clone();
            spawn_world(&mut commands, &ticker, &mut materials, &mut meshes, players, boomerangs, game_state);
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
        let mut current_tick = {
            let (_, _, _, _, ticker, _, players, _, mut meshes, mut materials, mut in_replay, _, _) = params.get_mut(world);
            ticker.0
        };
        while current_tick <= final_tick {
            {
                while !new_player_actions.is_empty() && new_player_actions.first().unwrap().0 == current_tick {
                    let (mut commands, client, mut query, lobjects, mut ticker, mut sent_events, _, _, _, _, _, _, _) =
                        params.get_mut(world);
                    let first = new_player_actions.first().unwrap();
                    {
                        record_player_action(&client, &ticker, &mut sent_events, &first.1, false);
                    }
                    apply_action_to_world(&first.1, world, &mut params);
                    new_player_actions.remove(0);
                    params.apply(world);
                }
                {
                    if !existing_records.is_empty() {
                        for player_action in existing_records.first().unwrap().player_actions.iter() {
                            apply_action_to_world(player_action, world, &mut params);
                        }
                    }
                }
            }
            // Run the full physics frame, not just the inner solver step. Avian's
            // Transform->Position sync (Prepare), clock advancement, and
            // Position->Transform writeback all live in FixedPostUpdate around the
            // PhysicsSchedule; running PhysicsSchedule alone simulates nothing visible.
            world.run_schedule(FixedPostUpdate);
            params.apply(world);
            if !existing_records.is_empty() {
                let (_, client, _, _, _, mut local_game_events, _, _, _, _, _, _, _) = params.get_mut(world);
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
                let (_, _, _, _, ticker, _, _, _, _, _, _, _, _) = params.get_mut(world);
                ticker.0
            };
        }
        {
            let (_, _, _, _, _, _, _, _, _, _, mut in_replay, _, _) = params.get_mut(world);
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

fn apply_action_to_world(
    player_action: &PlayerAction,
    world: &mut World,
    // The queries and resources this system used to take as individual params are
    // now pulled out of the `World` via a cached `SystemState`. Keeping it in a
    // `Local` preserves the query/change-detection state across frames instead of
    // rebuilding it every call.
    params: &mut Local<SystemState<(
        Commands,
        Res<GameClientWrapper>,
        Query<(Entity, &PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform, Option<&Children>), (With<PlayerId>, Without<Dying>, Without<Boomerang>)>,
        Query<(Entity, &Transform), (With<Boomerang>, Without<Swinging>)>,
        ResMut<Ticker>,
        ResMut<LocalGameEvents>,
        Query<Entity, With<PlayerId>>,
        Query<Entity, (With<Boomerang>, With<Thrown>)>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<InReplay>,
        ResMut<NextState<AppState>>,
        ResMut<PlayerActions>,
    )>>,
) {
    let (mut commands, client, mut query, lobjects, mut ticker, mut sent_events, _, _, _, _, _, _, mut player_actions) =
        params.get_mut(world);
    player_actions.actions.push(player_action.clone());
    params.apply(world);
}

pub fn move_player(
    mut players: Query<(&PlayerId, &mut Transform, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation), Without<Dead>>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::Movement { player_id, x, y } = player_action {
            for (player, mut transform, mut linear_velocity, mut constant_linear_acceleration, mut rotation) in &mut players {
                if player.player_id == *player_id {
                    linear_velocity.0.x = x.0;
                    linear_velocity.0.z = y.0;
                    *constant_linear_acceleration = ConstantLinearAcceleration(Vec3::new(x.0, 0.0, y.0).normalize_or_zero() * PLAYER_ACCEL);
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
                        transform.rotation = yaw;
                        rotation.0 = yaw;
                    }
                    break;
                }
            }
            return false;
        } else {
            return true;
        }
    });
}

pub fn swing_boomerang(
    mut commands: Commands,
    mut players: Query<(&PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, Option<&Children>)>,
    boomerangs: Query<(Entity, &Transform), (With<Boomerang>, Without<Swinging>)>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::Swing { player_id } = player_action {
            for (player, mut linear_velocity, mut constant_linear_acceleration, children) in &mut players {
                if player.player_id != *player_id { continue; }
                if let Some(children) = children {
                    for child in children.iter() {
                        if let Ok((boomerang, _)) = boomerangs.get(child) {
                            *linear_velocity = LinearVelocity(Vec3::ZERO);
                            *constant_linear_acceleration = ConstantLinearAcceleration(Vec3::ZERO);
                            commands.entity(boomerang).insert(Swinging { elapsed: 0.0 });
                        }
                    }
                }
            }
            return false;
        } else {
            return true;
        }
    });
}

pub fn jump_player(
    mut players: Query<(&PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform)>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::Jump { player_id, x, y } = player_action {
            for (player, mut linear_velocity, mut constant_linear_acceleration, mut transform) in &mut players {
                if player.player_id == *player_id {
                    linear_velocity.x = x.0;
                    linear_velocity.z = y.0;
                    linear_velocity.y = 2.0;
                    *constant_linear_acceleration = ConstantLinearAcceleration(Vec3::ZERO);
                    if Vec3::new(x.0, 0.0, y.0).length_squared() > 1e-6 {
                        let yaw = Quat::from_rotation_y(f32::atan2(-x.0, -y.0));
                        transform.rotation = yaw;
                    }
                }
            }
            return false;
        } else {
            return true;
        }
    });
}

pub fn wind_up_boomerang(
    mut commands: Commands,
    mut players: Query<(Entity, &PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform, Option<&Children>)>,
    boomerangs: Query<(), With<Boomerang>>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::StartingThrowing { player_id, x, y } = player_action {
            for (entity, player, mut linear_velocity, mut constant_linear_acceleration, mut transform, children) in &mut players {
                if player.player_id != *player_id { continue }
                linear_velocity.0 = Vec3::ZERO;
                constant_linear_acceleration.0 = Vec3::ZERO;
                if Vec3::new(x.0, 0.0, y.0).length_squared() > 1e-6 {
                    let yaw = Quat::from_rotation_y(f32::atan2(-x.0, -y.0));
                    transform.rotation = yaw;
                }
                let lobject_children: Vec<Entity> = if let Some(children) = children {
                    children.iter().filter(|child| { boomerangs.get(*child).is_ok() }).collect()
                } else {
                    vec![]
                };
                if !lobject_children.is_empty() {
                    commands.entity(entity).insert(StartThrow {});
                }
                break;
            }
            return false;
        } else {
            return true;
        }
    });
}

pub fn release_boomerang(
    mut commands: Commands,
    mut players: Query<(Entity, &PlayerId, &Transform, Option<&Children>)>,
    boomerangs: Query<Entity, With<Boomerang>>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::ReleaseThrow { player_id, power, x, y } = player_action {
            for (entity, player, player_transform, children) in &mut players {
                if player.player_id != *player_id { continue }
                commands.entity(entity).remove::<ThrowingAnimation>();
                if let Some(children) = children {
                    for child in children {
                        if let Ok(boomrang) = boomerangs.get(*child) {
                            let direction_vector = Vec3{x: x.0, y: 0.0, z: y.0}.normalize_or(Vec3 { x: 0.0, y: 0.0, z: -1.0 });
                            let new_origin_point = player_transform.translation + direction_vector * 1.0;
                            let offset_translation = Quat::from_rotation_y(PI/2.0).mul_vec3(direction_vector) * 0.5;
                            let mut new_transform = Transform::from_translation(new_origin_point + offset_translation).with_rotation(player_transform.rotation);
                            new_transform.rotate_around(new_origin_point, Quat::from_rotation_y(3.0*PI/4.0));
                            // let mut new_transform = Vec3{x:-0.5, y:0.0, z: -0.25});
                            // new_transform.rotate_around(new_transform.translation + Vec3{x: 0.5, y: 0.0, z: 0.0}, Quat::from_rotation_y(3.0*PI/4.0));
                            let velocity = LinearVelocity::from(Vec3{x: x.0, y: 0.0, z: y.0}.normalize_or_zero() * (4.0 + (power.0 * 8.0)));
                            let angular_velocity = AngularVelocity(Vec3{x:0.0, y:8.0, z:0.0});
                            println!("Transform: {:?}", new_transform);
                            println!("Velocity: {:?}", velocity);
                            commands.entity(entity).detach_child(boomrang);
                            commands.entity(boomrang).insert((
                                velocity,
                                Thrown{player_id: Some(*player_id)},
                                new_transform,
                                angular_velocity,
                                RigidBody::Dynamic,
                            ));
                        }
                    }
                }
                break;
            }
            return false;
        } else {
            return true;
        }
    });
}


/// Reads WASD/Gamepad input and drives the player's horizontal velocity, leaving the
/// vertical component to gravity / the physics solver.
///
/// Each local player picked a `Controller` in the lobby (`Keyboard` or a specific
/// `Gamepad`). We read every active input source, look up which local player it is
/// assigned to, and send a `Movement` event for that player's real id. The keyboard
/// is digital (full speed); the gamepad stick is analog (speed scales with tilt).
pub fn detect_player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    time: Res<Time>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    mut query: Query<(&mut PlayerId, &Transform, &LinearVelocity, &Rotation, &ConstantLinearAcceleration), (Without<Dying>, Without<Dead>)>,
    mut player_directions: ResMut<PlayerDirections>,
    player_move_cooldowns: Res<PlayerJumpCooldowns>,
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
    let now = std::time::SystemTime::now();
    for (player_id, velocity) in moves {
        for (mut player, ..) in &mut query {
            if player.player_id == player_id {
                if !player_directions.0.contains_key(&player_id) {
                    player_directions.0.insert(player_id, PlayerDirection { time: now, direction_facing: Vec3::ZERO, velocity: Vec3::ZERO });
                }
                let direction = player_directions.0.get_mut(&player_id).unwrap();
                let direction_changed = direction.velocity.clone() != velocity;
                let interval_elapsed = direction.time.elapsed().unwrap().as_millis() >= DIRECTION_EVENT_INTERVAL;
                // let interval_elapsed = false;
                if direction_changed || (!direction_changed && velocity != Vec3::ZERO && interval_elapsed) {
                    let player_move_cooldown = player_move_cooldowns.0.get(&player_id);
                    let proceed_with_move = match player_move_cooldown {
                        Some(player_cooldown) => *player_cooldown <= std::time::SystemTime::now(),
                        None => true,
                    };
                    if !proceed_with_move {
                        continue;
                    }
                    let mut direction_facing = velocity.normalize_or_zero();
                    direction_facing = if direction_facing.length_squared() < 0.01 {
                        direction.direction_facing
                    } else {
                        direction_facing
                    };
                    *direction = PlayerDirection { time: now, direction_facing: direction_facing, velocity };
                    direction.time = now;
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
    players: Query<(&PlayerId, &Children), (Without<Dying>, Without<Dead>)>,
    lobjects: Query<(), With<Boomerang>>,
    mut player_swing_cooldowns: ResMut<PlayerSwingCooldowns>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut swings = vec![];
    if keyboard.just_pressed(KeyCode::KeyZ) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, children) in &players {
                if player.player_id != *id { continue; }
                for child in children.iter() {
                    if lobjects.get(child).is_ok() {
                        swings.push(*id);
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
                    swings.push(*id);
                }
            }
        }
    }
    for id in swings {
        let player_move_cooldown = player_swing_cooldowns.0.get(&id);
        let proceed = match player_move_cooldown {
            Some(time) => {
                let now = std::time::SystemTime::now();
                *time<=now
            },
            None => true, 
        };
        if !proceed { continue; }
        player_swing_cooldowns.0.insert(id, std::time::SystemTime::now() + SWING_COOLDOWN);
        let game_event = PlayerAction::Swing { player_id: id };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_jump(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<&PlayerId, (Without<Dying>, Without<Dead>)>,
    player_directions: Res<PlayerDirections>,
    mut player_move_cooldowns: ResMut<PlayerJumpCooldowns>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut jumps = vec![];
    if keyboard.pressed(KeyCode::Space) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for player in &players {
                if player.player_id != *id { continue; }
                jumps.push(*id);
            }
        }
    }
    for (entity, gamepad) in &gamepads {
        if !gamepad.pressed(GamepadButton::South) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for player in &players {
            if player.player_id != *id { continue; }
            jumps.push(*id);
        }
    }
    for jump in jumps {
        let player_move_cooldown = player_move_cooldowns.0.get(&jump);
        let proceed = match player_move_cooldown {
            Some(time) => {
                let now = std::time::SystemTime::now();
                *time<=std::time::SystemTime::now()
            },
            None => true,
        };
        if !proceed { continue; }
        player_move_cooldowns.0.insert(jump, std::time::SystemTime::now() + JUMP_COOLDOWN);
        let player_direction = player_directions.0.get(&jump).unwrap().direction_facing;
        let player_direction = player_direction * 8.0;
        let game_event = PlayerAction::Jump { player_id: jump, x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_throwing(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<(&PlayerId, &Children), (Without<Dying>, Without<Dead>, Without<StartThrow>, Without<ThrowingAnimation>)>,
    boomerangs: Query<&Boomerang>,
    player_directions: Res<PlayerDirections>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut throwings = vec![];
    if keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, children) in &players {
                if player.player_id != *id { continue; }
                let boomerangs = children.iter().filter(|child| { boomerangs.get(*child).is_ok() }).collect::<Vec<Entity>>();
                if boomerangs.is_empty() { continue; }
                throwings.push(*id);
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if !gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for (player, children) in &players {
            if player.player_id != *id { continue; }
            let boomerangs = children.iter().filter(|child| { boomerangs.get(*child).is_ok() }).collect::<Vec<Entity>>();
            if boomerangs.is_empty() { continue; }
            throwings.push(*id);
        }
    }

    for throwing in throwings {
        let player_direction = player_directions.0.get(&throwing).unwrap().direction_facing;
        let player_direction = player_direction * 8.0;
        let game_event = PlayerAction::StartingThrowing { player_id: throwing, x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_pullback(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<(&PlayerId, Option<&Children>), (Without<Dying>, Without<Dead>, Without<StartThrow>, Without<ThrowingAnimation>)>,
    boomerangs: Query<&Boomerang>,
    mut players_pulling: ResMut<PlayersPulling>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut pulls = vec![];
    if keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, children) in &players {
                if player.player_id != *id { continue; }
                let boomerangs = children.map_or_else(|| { vec![] }, |children| { children.iter().filter(|child| { boomerangs.get(*child).is_ok() }).collect::<Vec<Entity>>() });
                if !boomerangs.is_empty() { continue; }
                pulls.push(*id);
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if !gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for (player, children) in &players {
            if player.player_id != *id { continue; }
            let boomerangs = children.map_or_else(|| { vec![] }, |children| { children.iter().filter(|child| { boomerangs.get(*child).is_ok() }).collect::<Vec<Entity>>() });
            if !boomerangs.is_empty() { continue; }
            pulls.push(*id);
        }
    }

    for throwing in pulls {
        let game_event = PlayerAction::StartingPulling { player_id: throwing };
        players_pulling.0.insert(throwing);
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn stop_pullback(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<&PlayerId, (Without<Dying>, Without<Dead>, Without<StartThrow>, Without<ThrowingAnimation>)>,
    boomerangs: Query<&Boomerang>,
    mut players_pulling: ResMut<PlayersPulling>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut pulls = vec![];
    if !keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for player in &players {
                if player.player_id != *id { continue }
                if !players_pulling.0.contains(id) { continue }
                pulls.push(*id);
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for player in &players {
            if player.player_id != *id { continue; }
            if !players_pulling.0.contains(id) { continue }
            pulls.push(*id);
        }
    }

    for throwing in pulls {
        let game_event = PlayerAction::StoppingPulling { player_id: throwing };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

pub fn release_throw(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<(&PlayerId, &ThrowingAnimation), (Without<Dying>, Without<Dead>, With<ThrowingAnimation>)>,
    player_directions: Res<PlayerDirections>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut throwings = vec![];
    if !keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, throwing) in &players {
                if player.player_id != *id { continue; }
                throwings.push((*id, throwing.elapsed.clamp(0.0, 2.0) / 2.0));
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for (player, throwing) in &players {
            if player.player_id != *id { continue; }
            throwings.push((*id, throwing.elapsed.clamp(0.0, 2.0) / 2.0));
        }
    }

    for throwing in throwings {
        let player_direction = player_directions.0.get(&throwing.0).unwrap().direction_facing;
        let player_direction = player_direction * 2.0;
        let game_event = PlayerAction::ReleaseThrow { player_id: throwing.0, power: OrderedF32(throwing.1), x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// When a player is newly marked `Dead`, relayer its body and blade colliders onto the
/// `Dead` layer (filtering to the `Platform` only) so the dead body still rests on the
/// floor but passes through every other player and boomerang. The hierarchy is fixed
/// depth-2 (player -> Boomerang -> blades), so we walk it directly.
pub fn apply_dead_collision_layers(
    newly_dead: Query<(Entity, &Children), Added<Dying>>,
    boomerangs: Query<&Children, With<Boomerang>>,
    mut commands: Commands,
) {
    let dead_layers = CollisionLayers::new(GameLayer::Dead, GameLayer::Environment);
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

pub fn pull_boomerang(
    mut thrown_boomerangs: Query<(&mut ConstantLinearAcceleration, &Thrown), (With<Boomerang>, With<Thrown>)>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::StoppingPulling { player_id } = player_action {
            for (mut constant_linear_acceleration, thrown) in &mut thrown_boomerangs {
                if thrown.player_id != Some(*player_id) { continue }
                *constant_linear_acceleration = ConstantLinearAcceleration(Vec3::ZERO);
                println!("acceleration: {:?}", constant_linear_acceleration);
            }
            return false;
        } else {
            return true;
        }
    });
}

pub fn stop_pulling_boomerang(
    players: Query<(&PlayerId, &Transform), (Without<Dying>, Without<Dead>)>,
    mut thrown_boomerangs: Query<(&mut ConstantLinearAcceleration, &Thrown, &Transform), (With<Boomerang>, With<Thrown>)>,
    mut player_actions: ResMut<PlayerActions>,
) {
    player_actions.actions.retain(|player_action| {
        if let PlayerAction::StartingPulling { player_id } = player_action {
            for (mut constant_linear_acceleration, thrown, boomerang_transform) in &mut thrown_boomerangs {
                println!("Pulling");
                if thrown.player_id != Some(*player_id) { continue }
                let destination = players.iter().filter(|player| { player.0.player_id == *player_id}).collect::<Vec<(&PlayerId, &Transform)>>();
                let Some((_, player_transform)) = destination.get(0) else { continue };
                let direction = (player_transform.translation - boomerang_transform.translation).normalize_or_zero() + Vec3{x:0.0, y: 9.8, z: 0.0};
                *constant_linear_acceleration = ConstantLinearAcceleration(direction);
                println!("acceleration: {:?}", constant_linear_acceleration);
            }
            return false;
        } else {
            return true;
        }
    });
}