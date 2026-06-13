use bevy::prelude::*;

use crate::app::screens::game_state::AppState;
use crate::server::{create_game_server, is_game_server_running};

/// Marks the root UI node of the menu screen, so it can be despawned on exit.
#[derive(Component)]
pub struct MenuRoot;

/// Marks the 2D camera used to render the menu UI, so it can be despawned on exit.
#[derive(Component)]
pub struct MenuCamera;

/// Marks the "Join Game" button so its clicks can be handled distinctly.
#[derive(Component)]
pub struct JoinGameButton;

/// Marks the "Create Game" button so its clicks can be handled distinctly.
#[derive(Component)]
pub struct CreateGameButton;

/// Marks the bottom-right text that reports whether the game server is running.
#[derive(Component)]
pub struct ServerStatusText;

/// Builds the menu screen: a 2D camera and a centered column with two buttons,
/// "Join Game" and "Create Game". The buttons are inert for now.
pub fn setup_menu(mut commands: Commands) {
    // Bevy UI needs a camera to render against; the 3D camera is only spawned
    // later once we enter `Playing`.
    commands.spawn((Camera2d::default(), MenuCamera));

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
            MenuRoot,
        ))
        .with_children(|parent| {
            for label in ["Join Game", "Create Game"] {
                let mut button = parent.spawn((
                    Button,
                    Node {
                        width: Val::Px(220.0),
                        height: Val::Px(60.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                ));
                match label {
                    "Join Game" => {
                        button.insert(JoinGameButton);
                    }
                    "Create Game" => {
                        button.insert(CreateGameButton);
                    }
                    _ => {}
                }
                button.with_children(|button| {
                    button.spawn((Text::new(label), TextColor(Color::WHITE)));
                });
            }
        });

    // Bottom-right status line reporting whether the global game server exists.
    // Spawned as its own absolutely-positioned node and tagged `MenuRoot` so the
    // existing `cleanup_menu` system despawns it on exit.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(10.0),
            right: Val::Px(10.0),
            ..default()
        },
        Text::new(server_status_label(is_game_server_running())),
        TextColor(Color::srgb(0.7, 0.7, 0.7)),
        ServerStatusText,
        MenuRoot,
    ));
}

/// Maps the server-running flag to the text shown in the status line.
fn server_status_label(running: bool) -> &'static str {
    if running {
        "Server: running"
    } else {
        "Server: stopped"
    }
}

/// Keeps the bottom-right status line in sync with whether the game server is running.
pub fn update_server_status_text(mut query: Query<&mut Text, With<ServerStatusText>>) {
    let label = server_status_label(is_game_server_running());
    for mut text in &mut query {
        if text.0 != label {
            text.0 = label.to_string();
        }
    }
}

/// Transitions to the `JoinGame` state when the "Join Game" button is pressed.
pub fn handle_join_game_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<JoinGameButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    for interaction in &interactions {
        println!("Clicked");
        if *interaction == Interaction::Pressed {
            next_state.set(AppState::JoinGame);
        }
    }
}

/// Transitions to the `CreateGame` state when the "Create Game" button is pressed.
pub fn handle_create_game_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<CreateGameButton>)>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            create_game_server();
        }
    }
}

/// Tears down the menu UI and its camera when leaving the menu state.
pub fn cleanup_menu(
    mut commands: Commands,
    menu: Query<Entity, With<MenuRoot>>,
    camera: Query<Entity, With<MenuCamera>>,
) {
    for entity in &menu {
        commands.entity(entity).despawn();
    }
    for entity in &camera {
        commands.entity(entity).despawn();
    }
}