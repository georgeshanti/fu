use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{app::{GameClientWrapper, screens::game_play::{animations::{Dying, Swinging}, entities::boomerang::{Boomerang, BoomerangBlade, Thrown}, state::{Dead, InReplay, LocalGameEvents, PlayerId, Ticker, record_game_effect}}}, server::GameEffect};

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_swing_strikes(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    players: Query<(Entity, &PlayerId), (Without<Dying>, Without<Dead>)>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
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
        if striker.1.player_id == struck.1.player_id {
            continue;
        }

        // Only the striker's owning client reports the strike.
        if !local_ids.contains(&striker.1.player_id) {
            continue;
        }

        let game_event = GameEffect::StrikePlayer {
            striker_id: striker.1.player_id,
            struck_id: struck.1.player_id,
        };
        commands.entity(struck.0).insert(Dying{elapsed: 0.0});
        record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event);
    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_throw_strikes(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    players: Query<(Entity, &PlayerId)>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    mut thrown: Query<(Entity, &Thrown, &mut Transform, &mut ConstantLinearAcceleration), With<Thrown>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
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
        let (blade_child_of, other_body) = match (blade1, blade2) {
            (Some(c), None) => (c, event.body2),
            (None, Some(c)) => (c, event.body1),
            (_, _) => { continue },
        };
        let Some(other_body) = other_body else { continue };

        let Ok(mut blade) = thrown.get_mut(blade_child_of.parent()) else {
            continue
        };
        let Ok(player) = players.get(other_body) else {
            continue
        };

        let Some(player_id) = blade.1.player_id else {continue};

        if player_id == player.1.player_id {
            println!("Here!!");
            commands.entity(blade.0).set_parent_in_place(player.0);
            commands.entity(blade.0).remove::<Thrown>();
            blade.2.translation = Vec3::ZERO;
        } else {
            let game_event = GameEffect::StrikePlayer {
                striker_id: player_id,
                struck_id: player.1.player_id,
            };
            commands.entity(player.0).insert(Dying{elapsed: 0.0});
            record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event);
        }
    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_parries(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    mut players: Query<(Entity, &mut LinearVelocity, &mut ConstantLinearAcceleration, &PlayerId, &Children, &Transform), Without<Boomerang>>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    mut lobjects: Query<(Entity, &mut Transform), (With<Boomerang>)>,
) {

    for event in collisions.read() {
        // Exactly one collider must be a blade. Neither -> a body-to-body bump;
        // both -> boomerang vs boomerang. Neither is a strike.
        let blade_1 = blades.get(event.collider1).ok();
        let blade_2 = blades.get(event.collider2).ok();
        let (Some(blade_1), Some(blade_2)) = (blade_1, blade_2) else { continue; };
        let (Some(player_1), Some(player_2)) = (event.body1, event.body2) else { continue; };

        // Swing gate: the blade's parent boomerang must be mid-swing.
        if swinging.get(blade_1.parent()).is_err() || swinging.get(blade_2.parent()).is_err(){
            continue;
        }

        let Ok([mut player_1, mut player_2]) = players.get_many_mut([player_1, player_2]) else {
            continue;
        };

        if player_1.3.player_id == player_2.3.player_id {
            continue;
        }

        let game_event = GameEffect::Parry {
            player_1_id: std::cmp::max(player_1.3.player_id, player_2.3.player_id),
            player_2_id: std::cmp::min(player_1.3.player_id, player_2.3.player_id),
        };

        for child in player_1.4.iter() {
            if let Ok((boomerang, mut transform)) = lobjects.get_mut(child) {
                commands.entity(boomerang).remove::<Swinging>();
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
            }
        }
        *player_1.1 = LinearVelocity((player_1.5.translation - player_2.5.translation).normalize() * 5.0);
        *player_1.2 = ConstantLinearAcceleration(Vec3::ZERO);

        for child in player_2.4.iter() {
            if let Ok((boomerang, mut transform)) = lobjects.get_mut(child) {
                commands.entity(boomerang).remove::<Swinging>();
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
            }
        }
        record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event.clone());
        *player_2.1 = LinearVelocity((player_2.5.translation - player_1.5.translation).normalize() * 5.0);
        *player_2.2 = ConstantLinearAcceleration(Vec3::ZERO);
    }
}