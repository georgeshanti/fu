use std::{collections::BTreeMap, time::{Duration, SystemTime}};

use bevy::prelude::*;

use crate::{app::{GameClientWrapper, screens::{app_state::AppState, game_play::{state::{PendingSpawns, PlayerInfos, ToBevyColor}, world::{GamePlayCamera, GamePlayRoot}}}}, server::{self, ClientEvent, ServerEvent}};


#[derive(Component)]
pub struct RoundEndTitle;

#[derive(Component)]
pub struct PointRow {
    player_id: u8,
    color: server::Color,
}

#[derive(Component)]
pub struct PointIndex {
    index: u8,
}

/// Root node of the countdown overlay (despawned when the countdown ends).
#[derive(Component)]
pub struct CountdownOverlay;

/// The big number `Text` inside the overlay.
#[derive(Component)]
pub struct CountdownText;

/// Root node of the round-ended overlay (despawned on `OnExit(AppState::RoundEnded)`).
#[derive(Component)]
pub struct RoundEndedOverlay;

/// The "Continue" button on the round-ended overlay: starts the next round.
#[derive(Component)]
pub struct ContinueButton;

#[derive(Resource)]
pub struct Score{
    pub time: SystemTime,
    pub max: u8,
    pub old_score: BTreeMap<u8, u8>,
    pub new_score: BTreeMap<u8, u8>,
    pub winners: Vec<u8>,
}

/// Fixed width of the scoreboard's name column, in logical pixels. Fixed (rather than
/// content-sized) so the point pips start at the same x on every row and read as columns.
const SCORE_NAME_COLUMN_WIDTH: f32 = 200.0;

/// Side length of one point pip. Square + `BorderRadius::MAX` (a radius of half the
/// smaller side) makes it a circle.
const SCORE_PIP_SIZE: f32 = 22.0;

/// Backing panel behind the scoreboard. Darker than the overlay's own 0.5 black so the
/// table reads as a distinct card against the frozen field showing through.
const SCORE_PANEL_BACKGROUND: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);

/// Puts the round-ended overlay up on `OnEnter(AppState::RoundEnded)`.
///
/// The 3D scene is deliberately left standing: every `AppState::Playing` system
/// (movement, swings, strike detection, event draining) stops running on the state
/// change, so the field simply freezes as it was and this dimmed UI layer draws on
/// top of it — same trick as the pre-game countdown overlay.
pub fn start_round_end_animation(mut commands: Commands, score: Option<Res<Score>>, player_infos: Res<PlayerInfos>) {
    // The server ends the round as soon as at most one player is still alive, so a
    // lone survivor is the winner; anything else (a same-tick double KO, or a round
    // ended some other way) is reported as a draw.

    // Full-screen dimmed overlay with the result centered on it.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(16.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            RoundEndedOverlay,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Round Over"),
                TextFont { font_size: 80.0, ..default() },
                TextColor(Color::WHITE),
                RoundEndTitle{},
            ));

            // Scoreboard, in its own translucent panel: one row per player, `max + 1`
            // columns — the name, then one pip per point of the round limit, filled up
            // to that player's score.
            let score = score.unwrap();
            parent
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::FlexStart,
                        row_gap: Val::Px(10.0),
                        padding: UiRect::axes(Val::Px(28.0), Val::Px(20.0)),
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        ..default()
                    },
                    BackgroundColor(SCORE_PANEL_BACKGROUND),
                ))
                .with_children(|table| {
                    for info in &player_infos.0 {
                        // A player with no entry in the map hasn't scored yet.
                        let points = score.old_score.get(&info.player_id).copied().unwrap_or(0);
                        table
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(10.0),
                                    ..default()
                                },
                                PointRow {
                                    player_id: info.player_id,
                                    color: info.color,
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    Text::new(info.name.clone()),
                                    TextFont { font_size: 28.0, ..default() },
                                    TextColor(info.color.to_bevy_color()),
                                    Node {
                                        width: Val::Px(SCORE_NAME_COLUMN_WIDTH),
                                        ..default()
                                    },
                                ));
                                for pip in 1..score.max+1 {
                                    row.spawn((
                                        PointIndex{index: pip},
                                        Node {
                                            width: Val::Px(SCORE_PIP_SIZE),
                                            height: Val::Px(SCORE_PIP_SIZE),
                                            border_radius: BorderRadius::MAX,
                                            ..default()
                                        },
                                        BackgroundColor(if pip <= points {
                                            info.color.to_bevy_color()
                                        } else {
                                            info.color.to_bevy_color().with_alpha(0.1)
                                        }),
                                    ));
                                }
                            });
                    }
                });
        });
}

pub fn setup_round_ended(
    mut commands: Commands,
    round_end_overlay: Query<Entity, With<RoundEndedOverlay>>,
    round_end_title: Query<&mut Text, With<RoundEndTitle>>,
    score: Res<Score>,
) {
    if score.winners.len() > 0 {
        for mut text in round_end_title {
            let mut winners = String::from("Winners:");
            let mut index = 0;
            for winner in score.winners.iter() {
                if index == 0 {
                    winners = format!("{} {},", winners, *winner);
                } else if index == score.winners.len() - 1 {
                    winners = format!("{} & {},", winners, *winner);
                } else {
                    winners = format!("{}, {},", winners, *winner);
                }
                index += 1;
            }
            *text = Text(winners);
        }
    }
    let game_ended = score.winners.len() > 0;
    for overlay in round_end_overlay {
        commands.entity(overlay).with_children(|parent| {
            // Action row, pinned to the bottom-right corner. The overlay root is itself
            // absolutely positioned at 100% x 100%, so this anchors against the whole
            // screen without disturbing the centered column above it. A flex row so more
            // buttons can sit alongside "Continue" later.
            parent
                .spawn(Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(24.0),
                    right: Val::Px(24.0),
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Button,
                        Node {
                            width: Val::Px(220.0),
                            height: Val::Px(60.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        ContinueButton,
                    ))
                    .with_children(|button| {
                        button.spawn((Text::new(if game_ended { "End Game" } else { "Continue" }), TextColor(Color::WHITE)));
                    });
                });
            });
    }
}

const POINT_ANIMATION_TIME: Duration = Duration::from_millis(500);

pub fn animate_round_end(
    rows: Query<(&PointRow, &Children)>,
    mut points: Query<(&PointIndex, &mut BackgroundColor)>,
    mut score: ResMut<Score>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if score.time.elapsed().unwrap() > POINT_ANIMATION_TIME {
        let player_ids: Vec<u8> = score.new_score.keys().map(|id| { *id }).collect();
        let mut index = 0;
        let player_id_score = loop {
            if index == player_ids.len() {
                break None;
            }
            let player_id = player_ids.get(index).unwrap();
            let new_player_score = *score.new_score.get(player_id).unwrap();
            if !score.old_score.contains_key(&player_id) {
                score.old_score.insert(*player_id, 0);
            }
            let old_player_score = score.old_score.get_mut(player_id).unwrap();
            if new_player_score != *old_player_score {
                *old_player_score += 1;
                break Some((*player_id, *old_player_score))
            }
            index += 1;
        };

        match player_id_score {
            Some((player_id, player_score)) => {
                for (point_row, children) in rows {
                    if point_row.player_id == player_id {
                        for child in children {
                            let Ok((point_index, mut background_color)) = points.get_mut(*child) else { continue };
                            if point_index.index == player_score {
                                *background_color = BackgroundColor(point_row.color.to_bevy_color());
                            }
                        }
                    }
                }
                score.time = SystemTime::now();
            },
            None => {
                next_state.set(AppState::RoundEnded);
            }
        };
    }
}

/// Asks the server for the next round when "Continue" is clicked. Any client may
/// press it; the server's phase guard collapses several clients' clicks into one
/// round, so no local dedupe is needed here.
pub fn handle_continue_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<ContinueButton>)>,
    client: Res<GameClientWrapper>,
    score: Res<Score>,
) {
    let event = if score.winners.len() > 0 {
        ClientEvent::EndGame
    } else {
        ClientEvent::MoveToNextRound
    };
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            let client_guard = client.client.read().unwrap();
            if let Some(sender) = &client_guard.sender {
                sender.send(event.clone()).ok();
            }
        }
    }
}

/// Drains server events while the round-ended overlay is up. Any client's "Continue"
/// makes the server rebroadcast `SpawnPlayers`, which sends every client back through
/// `SpawningPlayers` for the next round — the same handoff `update_lobby` does for the
/// first round.
///
/// Deliberately not registered for `RoundEndAnimation`: an event arriving mid-animation
/// just sits in `received_events` until this runs, and keeping the transition inside
/// `RoundEnded` guarantees `cleanup_round_ended` (on `OnExit`) always tears the scene down.
pub fn wait_for_next_round(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
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
        match event {
            ServerEvent::SpawnPlayers { spawns } => {
                commands.insert_resource(PendingSpawns(spawns));
                next_state.set(AppState::SpawningPlayers);
            }
            ServerEvent::BackToLobby => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                next_state.set(AppState::Lobby);
                return;
            },
            _ => {},
        }
    }
}

/// Despawns the round-ended overlay *and the playfield* on `OnExit(AppState::RoundEnded)`.
///
/// The scene has to survive `Playing → RoundEndAnimation → RoundEnded` (the overlay draws
/// on top of the frozen field), so this is the first point at which it can go. Player
/// bodies are left to `spawn_world`, which despawns every `PlayerId` entity before
/// rebuilding them. `OnExit` runs before the next state's `OnEnter`, so `setup_game_play`
/// replaces the camera in the same state transition and no frame renders without one.
pub fn cleanup_round_ended(
    mut commands: Commands,
    overlay: Query<Entity, With<RoundEndedOverlay>>,
    scene: Query<Entity, Or<(With<GamePlayRoot>, With<GamePlayCamera>)>>,
) {
    for entity in &overlay {
        commands.entity(entity).despawn();
    }
    for entity in &scene {
        commands.entity(entity).despawn();
    }
}