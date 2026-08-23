use std::{collections::{BTreeMap, BTreeSet}, f32::consts::PI, time::{Duration, SystemTime}};

use avian3d::{prelude::*};
use bevy::{ecs::{relationship::RelatedSpawnerCommands, system::SystemState}, prelude::*};

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, game_play::{actions::StartThrow, animations::{Dying, Swinging, ThrowingAnimation}, entities::boomerang::{Boomerang, Thrown}}}}, server::{self, ClientEvent, Controller, GameEffect, GameState, OrderedF32, Player, PlayerAction, PlayerBoomerangState, PlayerState, PlayerStatus, ServerEvent, ThrowingState, ThrownBoomerangeState},
};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
pub struct PlayerId {
    pub player_id: u8,
    pub color: server::Color,
}

pub struct PlayerInfo {
    pub player_id: u8,
    pub color: server::Color,
    pub name: String,
}

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Alive;

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Dead;

/// Remaining time on the pre-game countdown. Present only while counting down.
#[derive(Resource)]
pub struct Countdown {
    pub remaining: f32,
}

/// Counts `drain_server_events` invocations. Present only while `AppState::Playing`;
/// stamped onto outgoing `ClientEvent::GameEvent`s in place of the old hardcoded `tick: 0`.
#[derive(Resource, Default)]
pub struct Ticker(pub u64, pub bool);

/// Whether the client is currently replaying past ticks rather than playing live.
/// Present only while `AppState::Playing`; inserted alongside `Ticker`.
#[derive(Resource, Default)]
pub struct InReplay(pub bool);

#[derive(Resource, Default)]
pub struct PlayerInfos(pub Vec<PlayerInfo>);

#[derive(Clone)]
pub struct TickRecord {
    pub tick: u64,
    pub game_state: GameState,
    pub player_actions: BTreeSet<PlayerAction>,
    pub game_effects: BTreeSet<GameEffect>,
}

/// One entry per tick of local simulation recorded so far. Present only while
/// `AppState::Playing` (inserted alongside `Ticker`). Kept in non-decreasing `tick` order.
#[derive(Resource, Default)]
pub struct LocalGameEvents{
    pub base_tick: u64,
    pub game_events: Vec<TickRecord>
}

impl LocalGameEvents {
    /// Inserts a received (remote) `game_event` into the ledger keeping it ordered by `tick`.
    /// The snapshot is copied from the existing entry already recorded at that tick (the local
    /// state we simulated for it), or empty if none exists yet. The list is maintained in
    /// non-decreasing `tick` order, so a binary search finds the insertion point.
    pub fn insert_received_player_actions(&mut self, events: Vec<(u64, PlayerAction)>) -> Option<u64> {
        let mut lowest_tick = None;
        let mut pending_events: Vec<(u64, PlayerAction)> = vec![];
        for ticked_event in events {
            if let Some(current_lowest_tick) = lowest_tick {
                if current_lowest_tick < ticked_event.0 {
                    lowest_tick = Some(ticked_event.0);
                }
            } else {
                lowest_tick = Some(ticked_event.0);
            }
            self
                .game_events
                .iter_mut()
                .find(|e| e.tick == ticked_event.0)
                .map(|e| e.player_actions.insert(ticked_event.1))
                .unwrap_or_default();
            // Insert after any entries already at this tick, so the local same-tick snapshot
            // precedes the remote event.
        }
        lowest_tick
    }

    pub fn insert_received_game_effects(&mut self, events: Vec<(u64, GameEffect)>) -> Option<u64> {
        let mut lowest_tick = None;
        for ticked_event in events {
            if let Some(current_lowest_tick) = lowest_tick {
                if current_lowest_tick < ticked_event.0 {
                    lowest_tick = Some(ticked_event.0);
                }
            } else {
                lowest_tick = Some(ticked_event.0);
            }
            self
                .game_events
                .iter_mut()
                .find(|e| e.tick == ticked_event.0)
                .map(|e| e.game_effects.insert(ticked_event.1))
                .unwrap_or_default();
            // Insert after any entries already at this tick, so the local same-tick snapshot
            // precedes the remote event.
        }
        lowest_tick
    }

    pub fn add_state(&mut self, game_state: TickRecord) {
        self.game_events.push(game_state);
    }
}

/// Spawn assignments delivered by the server's `StartRound`, handed off to the
/// `SpawningPlayers` state. Inserted by `update_lobby`; consumed by the spawning screen.
#[derive(Resource)]
pub struct PendingSpawns(pub Vec<(Player, Vec3)>);

pub trait ToBevyColor {
    fn to_bevy_color(self: &Self) -> Color;
}

impl ToBevyColor for server::Color {
    fn to_bevy_color(self: &Self) -> Color {
        Color::srgb(
            self.red as f32 / 256.0,
            self.green as f32 / 256.0,
            self.blue as f32 / 256.0,
        )
    }
}

/// Records a `SentGameEvent` for the current tick if `move_player`/`start_swing`/
/// `detect_strikes` didn't already log one (i.e. no `GameEvent` was sent this tick).
/// Must run after those three systems so `sent_events`'s last tick reflects whether
/// they fired this frame.
pub fn record_tick_state(
    mut ticker: ResMut<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    player_query: Query<(&PlayerId, &Transform, &LinearVelocity, &ConstantLinearAcceleration, Option<&Children>, Option<&Dying>, Option<&Dead>, Option<&StartThrow>, Option<&ThrowingAnimation>)>,
    stationary_boomerangs: Query<&Boomerang, Without<Swinging>>,
    swinging_boomerangs: Query<(&Boomerang, &Swinging), With<Swinging>>,
    thrown_boomerangs: Query<(&Boomerang, &Thrown, Option<&LinearVelocity>, Option<&Transform>, Option<&ConstantLinearAcceleration>, Option<&AngularVelocity>), With<Thrown>>,
) {
    if ticker.1 {
        ticker.0 += 1;
    } else {
        ticker.1 = true;
    }
    if player_query.iter().len() == 0 {
        panic!("No players at tick: {}", ticker.0);
    }
    let game_state = GameState{
        players: player_query.iter()
        .map(|(player, transform, velocity, acceleration, children, dying, dead, start_throw, throwing)| {
            let mut player_boomerang_stationary: Option<PlayerBoomerangState> = None;
            if let Some(children) = children {
                for child in children {
                    if let Ok(_) = stationary_boomerangs.get(*child) {
                        player_boomerang_stationary = Some(PlayerBoomerangState::Stationary);
                        break;
                    }
                    if let Ok((_, swinging)) = swinging_boomerangs.get(*child) {
                        player_boomerang_stationary = Some(PlayerBoomerangState::Swinging { elapsed: swinging.elapsed });
                        break;
                    }
                }
            }
            PlayerState {
                status: match dying {
                    Some(dead) => PlayerStatus::Dying { elapsed: dead.elapsed },
                    None => match dead {
                        Some(_) => PlayerStatus::Dead {},
                        None => PlayerStatus::Alive,
                    },
                },
                player_id: player.player_id,
                color: player.color,
                position: transform.translation,
                velocity: velocity.0,
                rotation: transform.rotation,
                acceleration: acceleration.0,
                bommerang: player_boomerang_stationary,
                throwing_state: match start_throw {
                    Some(start_throw) => Some(ThrowingState::StartThrow {}),
                    None => match throwing {
                        Some(throwing) => Some(ThrowingState::Throwing { elapsed: throwing.elapsed }),
                        None => None,
                    }
                }
            }
        })
        .collect(),
        // thrown_boomerangs: vec![],
        thrown_boomerangs: thrown_boomerangs.iter().map(|(boomerang, thrown, linear_velocity, transform, acceleration, angular_velocity)| {
            ThrownBoomerangeState {
                player_id: thrown.player_id,
                position: transform.map_or_else(|| { Vec3::ZERO }, |transform| {transform.translation}),
                velocity: linear_velocity.map_or_else(|| { Vec3::ZERO }, |linear_velocity| {linear_velocity.0}),
                rotation: transform.map_or_else(|| { Quat::IDENTITY }, |transform| {transform.rotation}),
                acceleration: acceleration.map_or_else(|| { Vec3::ZERO }, |acceleration| {acceleration.0}),
                angular_veloctiy: angular_velocity.map_or_else(|| { Vec3::ZERO }, |angular_velocity| {angular_velocity.0}),
            }
        }).collect(),
    };
    sent_events.add_state(TickRecord {
        tick: ticker.0,
        game_state: game_state,
        player_actions: BTreeSet::new(),
        game_effects: BTreeSet::new(),
    });
}

pub fn record_player_action(
    client: &Res<GameClientWrapper>,
    ticker: &Ticker,
    sent_events: &mut ResMut<LocalGameEvents>,
    player_action: &PlayerAction,
    send_to_server: bool,
) {
    if let Some(sender) = &client.client.read().unwrap().sender {
        sent_events.insert_received_player_actions( vec![ (ticker.0, player_action.clone()) ]);
        if send_to_server {
            sender
                .send(ClientEvent::PlayerAction { tick: ticker.0, game_event: player_action.clone() })
                .ok();
        }
    }
}

pub fn record_game_effect(
    in_replay: &Res<InReplay>,
    client: &Res<GameClientWrapper>,
    ticker: &Res<Ticker>,
    sent_events: &mut ResMut<LocalGameEvents>,
    game_effect: GameEffect,
) {
    if let Some(sender) = &client.client.read().unwrap().sender {
        sent_events.insert_received_game_effects( vec![ (ticker.0, game_effect.clone()) ]);
        if !in_replay.0 {
            sender
                .send(ClientEvent::GameEffect { tick: ticker.0, game_event: game_effect })
                .ok();
        }
    }
}