use std::{sync::{Arc, RwLock}, thread};

use bevy::prelude::*;

use crate::{app::{GameClientWrapper, common::text::{InputField, InputText, TextInputFocused}, screens::game_state::AppState}, client::GameClient, connection::client::create_client, server::{CLIENT_EVENT_SENDER, GAME_SERVER, is_game_server_running}};

/// Marks the root UI node of the join-game screen, so it can be despawned on exit.
#[derive(Component)]
pub struct JoinScreenRoot;

/// Marks the 2D camera used to render the join-game screen.
#[derive(Component)]
pub struct JoinScreenCamera;

/// Marks the "Join" button on the join-game screen.
#[derive(Component)]
pub struct JoinOnlineServerButton;

/// Marks the "Join Local Server" button, shown only when a global game server is running.
#[derive(Component)]
pub struct JoinLocalServerButton;

/// Builds the join-game screen: a 2D camera, a text input field, and a "Join"
/// button. Typing is handled by `update_join_input`; the button starts the game.
pub fn setup_join_screen(mut commands: Commands) {
    commands.spawn((Camera2d::default(), JoinScreenCamera));

    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            JoinScreenRoot,
        ))
        .with_children(|parent| {
            // One labeled text field per entry; the first starts focused. Each
            // field's contents live in its `JoinInputField`; the child `Text`
            // (tagged `JoinInputText`) mirrors them for display. Fields are
            // `Button`s so clicks move keyboard focus between them.
            for (label, focused) in [("Player Name", true), ("Game Code", false)] {
                parent.spawn((
                    Text::new(label),
                    TextColor(Color::srgb(0.7, 0.7, 0.7)),
                ));

                let mut field = parent.spawn((
                    Button,
                    Node {
                        width: Val::Px(300.0),
                        height: Val::Px(50.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.15, 0.15, 0.18)),
                    InputField::default(),
                ));
                if focused {
                    field.insert(TextInputFocused);
                }
                field.with_children(|field| {
                    field.spawn((Text::new(""), TextColor(Color::WHITE), InputText));
                });
            }

            // Join button.
            parent
                .spawn((
                    Button,
                    Node {
                        width: Val::Px(220.0),
                        height: Val::Px(60.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                    JoinOnlineServerButton,
                ))
                .with_children(|button| {
                    button.spawn((Text::new("Join Online"), TextColor(Color::WHITE)));
                });

            // "Join Local Server" button: only shown when a global (in-process) game
            // server is running, offering a one-click join to that embedded server.
            if is_game_server_running() {
                parent
                    .spawn((
                        Button,
                        Node {
                            width: Val::Px(220.0),
                            height: Val::Px(60.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        JoinLocalServerButton,
                    ))
                    .with_children(|button| {
                        button.spawn((Text::new("Join Local Server"), TextColor(Color::WHITE)));
                    });
            }
        });
}

/// Starts the game when the "Join" button is pressed.
pub fn handle_join_online_submit_button(
    mut commands: Commands,
    interactions: Query<&Interaction, (Changed<Interaction>, With<JoinOnlineServerButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
        let (game_client, sender) = GameClient::new();
        let game_client = GameClientWrapper{client: Arc::new(RwLock::new(game_client))};

        let (request_sender, response_receiver) = create_client(String::from("127.0.0.1:8765"));

        game_client.client.write().unwrap().attach_sender(request_sender);
        thread::spawn(move || {
            loop {
                let response = response_receiver.recv().unwrap();
                sender.send(response);
            }
        });

        game_client.client.read().unwrap().start_client();
        commands.insert_resource(game_client);
            next_state.set(AppState::Playing);
        }
    }
}

/// Joins the in-process game server when the "Join Local Server" button is pressed.
pub fn handle_join_local_server_button(
    mut commands: Commands,
    interactions: Query<&Interaction, (Changed<Interaction>, With<JoinLocalServerButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
        let (game_client, sender) = GameClient::new();
        let game_client = GameClientWrapper{client: Arc::new(RwLock::new(game_client))};
        {
            GAME_SERVER.lock().unwrap().as_mut().unwrap().attach_sender(sender);
        }
        game_client.client.write().unwrap().attach_sender(CLIENT_EVENT_SENDER.lock().unwrap().as_ref().unwrap().clone());
        game_client.client.read().unwrap().start_client();
        commands.insert_resource(game_client);
            next_state.set(AppState::Playing);
        }
    }
}

/// Tears down the join-game screen and its camera when leaving the join state.
pub fn cleanup_join_screen(
    mut commands: Commands,
    root: Query<Entity, With<JoinScreenRoot>>,
    camera: Query<Entity, With<JoinScreenCamera>>,
) {
    for entity in &root {
        commands.entity(entity).despawn();
    }
    for entity in &camera {
        commands.entity(entity).despawn();
    }
}