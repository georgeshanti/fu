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

/// Counts simulated fixed-update ticks (`drain_server_events` invocations). Present only
/// while `AppState::Playing`. Events produced by the Update-schedule input systems are
/// stamped `Ticker + 1` — the next tick to be simulated — since the current tick's physics
/// step has already run by the time they fire.
#[derive(Resource, Default)]
pub struct Ticker(pub u64);

/// How many ticks of simulation history the ledger keeps for rollback: ~5 s at Bevy's
/// default 64 Hz fixed timestep. An event older than the window is applied at the oldest
/// retained tick instead of its true tick (better late than never).
const ROLLBACK_WINDOW_TICKS: usize = 320;

/// A snapshot of one player's physics at the start of a tick. Uses Avian's `Position`/
/// `Rotation` (the physics source of truth), not `Transform` — the Transform syncs run
/// outside `PhysicsSchedule`, so during replay only the Avian components are current.
/// `acceleration` mirrors `ConstantLinearAcceleration`: it persists between `Movement`
/// events, so a rolled-back player whose last `Movement` predates the rollback point
/// would otherwise replay with the wrong accel.
#[derive(Clone)]
pub struct PlayerState {
    pub player_id: u8,
    pub position: Vec3,
    pub velocity: Vec3,
    pub rotation: Quat,
    pub acceleration: Vec3,
}

/// One simulated tick: the state of every player at the *start* of the tick (before the
/// tick's events are applied and before its physics step), plus the events to apply at
/// this tick. Replaying tick k = "apply `events`, run one physics step", which produces
/// the snapshot for tick k+1.
pub struct TickRecord {
    pub snapshot: Vec<PlayerState>,
    pub events: Vec<GameEvent>,
}

/// Dense per-tick simulation history: `records[i]` is tick `base_tick + i`. Present only
/// while `AppState::Playing` (inserted alongside `Ticker`). Holds both events this client
/// sent and events received from the server; identical events at the same tick are stored
/// once, which is how a client's own echoed-back events are recognized and ignored.
#[derive(Resource, Default)]
pub struct TickLedger {
    pub base_tick: u64,
    pub records: Vec<TickRecord>,
    /// Events for ticks the local simulation hasn't reached yet (client tickers are not
    /// synchronized, so a received tick can be in the local future; local input is also
    /// recorded one tick ahead). Moved into `records` by `begin_tick` when due.
    pub deferred: Vec<(u64, GameEvent)>,
}

impl TickLedger {
    /// Tick of the newest record. Only meaningful when `records` is non-empty.
    fn last_tick(&self) -> u64 {
        self.base_tick + self.records.len() as u64 - 1
    }

    /// Opens the record for `tick` with the given start-of-tick snapshot, pulling in any
    /// deferred events that are now due. Called exactly once per tick, with consecutive
    /// tick values, so the records stay dense.
    pub fn begin_tick(&mut self, tick: u64, snapshot: Vec<PlayerState>) {
        if self.records.is_empty() {
            self.base_tick = tick;
        }
        let mut events = Vec::new();
        self.deferred.retain(|(t, event)| {
            if *t <= tick {
                events.push(event.clone());
                false
            } else {
                true
            }
        });
        self.records.push(TickRecord { snapshot, events });
    }

    /// Records `event` at `tick`: into the tick's record (clamped to the retained window),
    /// or into `deferred` if the tick hasn't been simulated yet. Returns the tick it was
    /// stored at, or `None` if an identical event is already recorded there — the dedupe
    /// that keeps this client's own events, echoed back by the server, from re-applying.
    pub fn push_event(&mut self, tick: u64, event: GameEvent) -> Option<u64> {
        if self.records.is_empty() || tick > self.last_tick() {
            if self.deferred.iter().any(|(t, e)| *t == tick && *e == event) {
                return None;
            }
            self.deferred.push((tick, event));
            return Some(tick);
        }
        let tick = tick.max(self.base_tick);
        let record = &mut self.records[(tick - self.base_tick) as usize];
        if record.events.contains(&event) {
            return None;
        }
        record.events.push(event);
        Some(tick)
    }

    /// Inserts events received from the server. Returns the lowest tick that gained a
    /// genuinely new event *in the local past* (strictly before `current_tick`) — the
    /// tick to roll back to — or `None` if nothing new landed in the past.
    pub fn insert_received(&mut self, current_tick: u64, events: Vec<(u64, GameEvent)>) -> Option<u64> {
        let mut lowest_new_past_tick = None;
        for (tick, event) in events {
            let Some(stored_tick) = self.push_event(tick, event) else {
                continue;
            };
            if stored_tick < current_tick
                && lowest_new_past_tick.map_or(true, |lowest| stored_tick < lowest)
            {
                lowest_new_past_tick = Some(stored_tick);
            }
        }
        lowest_new_past_tick
    }

    /// Drops records older than `ROLLBACK_WINDOW_TICKS` so the ledger stays bounded.
    pub fn prune(&mut self) {
        let excess = self.records.len().saturating_sub(ROLLBACK_WINDOW_TICKS);
        if excess > 0 {
            self.records.drain(..excess);
            self.base_tick += excess as u64;
        }
    }
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
            commands.insert_resource(TickLedger::default());
            next_state.set(AppState::Playing);
        }
    }
}

/// Snapshots every player's physics state from Avian's `Position`/`Rotation` (current
/// even mid-replay, unlike `Transform` — the Transform syncs run outside `PhysicsSchedule`).
fn snapshot_players(world: &mut World) -> Vec<PlayerState> {
    let mut query = world.query::<(&Player, &Position, &Rotation, &LinearVelocity, &ConstantLinearAcceleration)>();
    query
        .iter(world)
        .map(|(player, position, rotation, velocity, accel)| PlayerState {
            player_id: player.player_id,
            position: position.0,
            velocity: velocity.0,
            rotation: rotation.0,
            acceleration: accel.0,
        })
        .collect()
}

/// Restores every snapshotted player's physics state. Writes Avian's `Position`/`Rotation`
/// directly — the physics source of truth — since the Transform→Position sync won't run
/// before the manual replay steps. Players missing from the snapshot are left untouched.
fn restore_players(world: &mut World, snapshot: &[PlayerState]) {
    let mut query = world.query::<(&Player, &mut Position, &mut Rotation, &mut LinearVelocity, &mut ConstantLinearAcceleration)>();
    for (player, mut position, mut rotation, mut velocity, mut accel) in query.iter_mut(world) {
        if let Some(state) = snapshot.iter().find(|s| s.player_id == player.player_id) {
            position.0 = state.position;
            *rotation = Rotation(state.rotation);
            velocity.0 = state.velocity;
            accel.0 = state.acceleration;
        }
    }
}

/// Copies each player's replayed `Position`/`Rotation` back onto its `Transform`. Avian's
/// `Writeback` sync only runs for the normally-scheduled step, so after manual replay
/// steps the Transforms are stale; left that way, the next `Prepare` (Transform→Position)
/// sync would clobber the corrected physics state with the pre-rollback one.
fn mirror_physics_to_transforms(world: &mut World) {
    let mut query = world.query_filtered::<(&Position, &Rotation, &mut Transform), With<Player>>();
    for (position, rotation, mut transform) in query.iter_mut(world) {
        transform.translation = position.0;
        transform.rotation = rotation.0;
    }
}

/// Applies one `GameEvent` to the local simulation. The single application path for both
/// the live tick and rollback replay, so they can't diverge.
fn apply_game_event(world: &mut World, game_event: &GameEvent) {
    match game_event {
        GameEvent::Movement { player_id, x, y } => {
            let mut query = world.query_filtered::<(&Player, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation), Without<Dead>>();
            for (player, mut vel, mut accel, mut rotation) in query.iter_mut(world) {
                if player.player_id == *player_id {
                    vel.x = *x;
                    vel.z = *y;
                    *accel = ConstantLinearAcceleration(Vec3::new(*x, 0.0, *y).normalize_or_zero() * PLAYER_ACCEL);
                    // Point the player (and its anchored L) toward the movement
                    // direction. Forward is -Z, so yaw = atan2(-x, -z). Leave the
                    // facing unchanged when stationary.
                    if Vec3::new(*x, 0.0, *y).length_squared() > 1e-6 {
                        *rotation = Quat::from_rotation_y(f32::atan2(-*x, -*y)).into();
                    }
                }
            }
        }
        GameEvent::Swing { player_id } => {
            let mut boomerangs = Vec::new();
            let mut query = world.query_filtered::<(&Player, &Children), Without<Dead>>();
            for (player, children) in query.iter(world) {
                if player.player_id != *player_id {
                    continue;
                }
                boomerangs.extend(children.iter());
            }
            for child in boomerangs {
                let entity = world.entity(child);
                // Mirror the live gating: only a resting boomerang starts a swing, so a
                // replayed Swing doesn't restart one already in flight.
                if entity.contains::<Boomerang>() && !entity.contains::<Swinging>() {
                    world.entity_mut(child).insert(Swinging { elapsed: 0.0 });
                }
            }
        }
        GameEvent::StrikePlayer { struck_id, .. } => {
            // Mark the struck player dead; `animate_death` shrinks it and
            // `apply_dead_collision_layers` relayers it to touch only the platform.
            let mut struck = Vec::new();
            let mut query = world.query_filtered::<(Entity, &Player), Without<Dead>>();
            for (entity, player) in query.iter(world) {
                if player.player_id == *struck_id {
                    struck.push(entity);
                }
            }
            for entity in struck {
                world.entity_mut(entity).insert(Dead { elapsed: 0.0 });
            }
        }
    }
}

/// The per-tick heart of the client simulation, run in `FixedUpdate` just before Avian
/// steps in `FixedPostUpdate`. Advances the `Ticker`, snapshots the start-of-tick state
/// into the `TickLedger`, and drains the network buffer. A received event whose tick is
/// already in the local past triggers a rollback: restore that tick's snapshot, then
/// re-simulate up to the present — re-applying each tick's ledger events and manually
/// running one `PhysicsSchedule` step per tick, overwriting the now-stale snapshots.
/// Finally the current tick's events (received for now, deferred-until-now, or recorded
/// by last frame's input systems) are applied, ahead of this tick's normal physics step.
///
/// Exclusive (`&mut World`) because replay must run `PhysicsSchedule` mid-system.
pub fn drain_server_events(world: &mut World) {
    // Advance the tick and open its ledger record with the start-of-tick snapshot.
    let tick = {
        let mut ticker = world.resource_mut::<Ticker>();
        ticker.0 += 1;
        ticker.0
    };
    let snapshot = snapshot_players(world);
    world.resource_mut::<TickLedger>().begin_tick(tick, snapshot);

    // Drain the network buffer and ledger the game events. Other `ServerEvent`s are not
    // handled during play, as before.
    let events = {
        let client = world.resource::<GameClientWrapper>().client.clone();
        let client = client.read().unwrap();
        let mut server_events = client.received_events.lock().unwrap();
        std::mem::take(&mut *server_events)
    };
    let game_events = events
        .into_iter()
        .filter_map(|event| {
            if let ServerEvent::GameEvent { tick, game_event } = event {
                Some((tick, game_event))
            } else {
                None
            }
        })
        .collect();
    let rollback_tick = world
        .resource_mut::<TickLedger>()
        .insert_received(tick, game_events);

    // Rollback + replay when a genuinely new event landed in the local past.
    if let Some(rollback_tick) = rollback_tick {
        let restore_snapshot = {
            let ledger = world.resource::<TickLedger>();
            ledger.records[(rollback_tick - ledger.base_tick) as usize].snapshot.clone()
        };
        restore_players(world, &restore_snapshot);

        let timestep = world.resource::<Time<Fixed>>().timestep();
        for replay_tick in rollback_tick..tick {
            let replay_events = {
                let ledger = world.resource::<TickLedger>();
                ledger.records[(replay_tick - ledger.base_tick) as usize].events.clone()
            };
            for event in &replay_events {
                apply_game_event(world, event);
            }
            // One manual physics step: `PhysicsSchedule` consumes `Time<Physics>`'s delta,
            // which must be advanced by hand when the schedule is run outside its driver.
            // (Replayed steps re-emit CollisionStart messages, so `detect_strikes` may
            // re-report strikes — benign, the server dedupes on the alive→dead transition.)
            world.resource_mut::<Time<Physics>>().advance_by(timestep);
            world.run_schedule(PhysicsSchedule);
            let snapshot = snapshot_players(world);
            let mut ledger = world.resource_mut::<TickLedger>();
            let base_tick = ledger.base_tick;
            ledger.records[(replay_tick + 1 - base_tick) as usize].snapshot = snapshot;
        }
        mirror_physics_to_transforms(world);
    }

    // Apply the current tick's events before this tick's physics step.
    let current_events = {
        let ledger = world.resource::<TickLedger>();
        ledger.records[(tick - ledger.base_tick) as usize].events.clone()
    };
    for event in &current_events {
        apply_game_event(world, event);
    }

    world.resource_mut::<TickLedger>().prune();
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
    mut ledger: ResMut<TickLedger>,
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
                        // Recorded (and sent) for the *next* tick: this tick's physics
                        // already ran, so `drain_server_events` applies it at the top of
                        // tick+1 — the same path a replay takes, and the server's echo
                        // of it dedupes against this ledger entry instead of rolling back.
                        let game_event = GameEvent::Movement { player_id, x: velocity.x, y: velocity.z };
                        ledger.push_event(ticker.0 + 1, game_event.clone());
                        sender.send(ClientEvent::GameEvent { tick: ticker.0 + 1, game_event }).ok();
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
    mut ledger: ResMut<TickLedger>,
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
                            // Recorded and sent for the next tick — see `move_player`.
                            let game_event = GameEvent::Swing { player_id: *id };
                            ledger.push_event(ticker.0 + 1, game_event.clone());
                            sender.send(ClientEvent::GameEvent {
                                tick: ticker.0 + 1,
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
                        // Recorded and sent for the next tick — see `move_player`.
                        let game_event = GameEvent::Swing { player_id: *id };
                        ledger.push_event(ticker.0 + 1, game_event.clone());
                        sender.send(ClientEvent::GameEvent {
                            tick: ticker.0 + 1,
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
    mut ledger: ResMut<TickLedger>,
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
            // Recorded and sent for the next tick — see `move_player`.
            let game_event = GameEvent::StrikePlayer {
                striker_id: striker.player_id,
                struck_id: struck.player_id,
            };
            ledger.push_event(ticker.0 + 1, game_event.clone());
            sender
                .send(ClientEvent::GameEvent { tick: ticker.0 + 1, game_event })
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

