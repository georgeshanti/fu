use bevy::prelude::*;

use crate::{
    app::{
        GameClientWrapper,
        common::text::{InputField, InputText, TextInputFocused},
        screens::app_state::AppState,
    },
    client::ClientPlayer,
    server::{ClientEvent, Controller, Player, ServerEvent},
};

/// Spawn assignments delivered by the server's `StartRound`, handed off to the
/// `SpawningPlayers` state. Inserted by `update_lobby`; consumed by the spawning screen.
#[derive(Resource)]
pub struct PendingSpawns(pub Vec<(Player, Vec3)>);

/// Marks the root UI node of the lobby screen, so it can be despawned on exit.
#[derive(Component)]
pub struct LobbyRoot;

/// Marks the 2D camera used to render the lobby screen.
#[derive(Component)]
pub struct LobbyCamera;

/// Marks the node that holds one row per connected player.
#[derive(Component)]
pub struct PlayerListContainer;

/// Marks a single player row, so the list can be rebuilt on each `LobbyInfo`.
#[derive(Component)]
pub struct PlayerRow;

/// Marks the name input field used to join the lobby.
#[derive(Component)]
pub struct LobbyNameField;

/// Marks the "Join" button on the lobby screen.
#[derive(Component)]
pub struct LobbyJoinButton;

/// Marks the "Start Game" button on the lobby screen.
#[derive(Component)]
pub struct LobbyStartButton;

/// The controller the player has chosen in the lobby dropdown, or `None` when no
/// input method is available. Read by `handle_lobby_join_button` and reset to
/// `None` on each lobby entry (`populate_controller_options` picks the first
/// available option on the next frame).
#[derive(Resource, Default)]
pub struct SelectedController(pub Option<Controller>);

/// Marks the root of the controller dropdown (header + collapsible panel).
#[derive(Component)]
pub struct ControllerDropdownRoot;

/// Tracks whether the dropdown panel is currently expanded.
#[derive(Component, Default)]
pub struct DropdownOpen(pub bool);

/// Marks the always-visible button that toggles the dropdown open/closed.
#[derive(Component)]
pub struct ControllerDropdownHeader;

/// Marks the `Text` inside the header, updated to show the current selection.
#[derive(Component)]
pub struct ControllerDropdownHeaderText;

/// Marks the collapsible panel that holds the option buttons.
#[derive(Component)]
pub struct ControllerDropdownPanel;

/// A single clickable option button; carries the `Controller` it selects.
#[derive(Component)]
pub struct ControllerOption(pub Controller);

/// Human-readable label for a controller, reused by the dropdown header and the
/// player roster rows.
fn controller_label(controller: Controller) -> String {
    match controller {
        Controller::Keyboard => "Keyboard".to_string(),
        Controller::Gamepad(n) => format!("Gamepad #{n}"),
    }
}

/// Builds the lobby screen and asks the server for the current roster by sending
/// a `FetchLobby` event; the reply (`LobbyInfo`) is rendered by `update_lobby`.
pub fn setup_lobby(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    mut selected: ResMut<SelectedController>,
) {
    commands.spawn((Camera2d::default(), LobbyCamera));

    // Clear the selection; `populate_controller_options` picks the first
    // available input method on the next frame.
    selected.0 = None;

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
            LobbyRoot,
        ))
        .with_children(|parent| {
            parent.spawn((Text::new("Lobby"), TextColor(Color::WHITE)));

            parent.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(8.0),
                    ..default()
                },
                PlayerListContainer,
            ));

            // Name input field (starts focused) used to join the lobby.
            parent
                .spawn((
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
                    LobbyNameField,
                    TextInputFocused,
                ))
                .with_children(|field| {
                    field.spawn((Text::new(""), TextColor(Color::WHITE), InputText));
                });

            // Controller dropdown: a header button showing the current selection
            // and a collapsible panel of options, populated by
            // `populate_controller_options` from the live set of gamepads.
            parent
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(4.0),
                        ..default()
                    },
                    ControllerDropdownRoot,
                    DropdownOpen(false),
                ))
                .with_children(|dropdown| {
                    // Header (always visible) — click to expand/collapse.
                    dropdown
                        .spawn((
                            Button,
                            Node {
                                width: Val::Px(300.0),
                                height: Val::Px(50.0),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.15, 0.15, 0.18)),
                            ControllerDropdownHeader,
                        ))
                        .with_children(|header| {
                            header.spawn((
                                Text::new(controller_label(Controller::Keyboard)),
                                TextColor(Color::WHITE),
                                ControllerDropdownHeaderText,
                            ));
                        });

                    // Collapsible options panel (starts hidden via Display::None).
                    dropdown.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(4.0),
                            display: Display::None,
                            ..default()
                        },
                        ControllerDropdownPanel,
                    ));
                });

            // "Join" button: emits a JoinLobby event with the typed name.
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
                    LobbyJoinButton,
                ))
                .with_children(|button| {
                    button.spawn((Text::new("Join"), TextColor(Color::WHITE)));
                });

            // "Start Game" button: inert placeholder for now.
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
                    LobbyStartButton,
                ))
                .with_children(|button| {
                    button.spawn((Text::new("Start Game"), TextColor(Color::WHITE)));
                });
        });

    // Ask the server who is connected; the response is handled by `update_lobby`.
    if let Some(sender) = &client.client.read().unwrap().sender {
        sender.send(ClientEvent::FetchLobby).ok();
    }
}

/// Drains server events and, on a `LobbyInfo`, rebuilds the player list to show
/// each connected player's name and id.
pub fn update_lobby(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    container: Query<Entity, With<PlayerListContainer>>,
    rows: Query<Entity, With<PlayerRow>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let events = {
        let client = client.client.read().unwrap();
        let mut server_events = client.received_events.lock().unwrap();
        let events = server_events.clone();
        *server_events = vec![];
        events
    };

    for event in events {
        match event {
            ServerEvent::LobbyInfo { players } => {
                // Update this client's own player list: the roster entries whose
                // `client_id` matches this client's assigned id.
                {
                    let client = client.client.read().unwrap();
                    if let Some(own_id) = *client.client_id.read().unwrap() {
                        let mine = players
                            .iter()
                            .filter(|p| p.client_id == own_id)
                            .map(|p| ClientPlayer { id: p.id, name: p.name.clone(), controller: p.controller })
                            .collect();
                        *client.players.write().unwrap() = mine;
                    }
                }

                // Clear the previous roster, then render the latest one.
                for row in &rows {
                    commands.entity(row).despawn();
                }
                let Ok(container) = container.single() else {
                    continue;
                };
                for Player { id, client_id, name, controller } in players {
                    let row = commands
                        .spawn((
                            Text::new(format!(
                                "{name}  (#{id}, client {client_id}) — {}",
                                controller_label(controller)
                            )),
                            TextColor(Color::WHITE),
                            PlayerRow,
                        ))
                        .id();
                    commands.entity(container).add_child(row);
                }
            },
            ServerEvent::SpawnPlayers { spawns } => {
                commands.insert_resource(PendingSpawns(spawns));
                next_state.set(AppState::SpawningPlayers);
            }
            _ => {},
        }
    }
}

/// Spawns a single dropdown option button carrying its `Controller` value.
fn spawn_option_button(commands: &mut Commands, label: String, controller: Controller) -> Entity {
    commands
        .spawn((
            Button,
            Node {
                width: Val::Px(300.0),
                height: Val::Px(40.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.12, 0.12, 0.14)),
            ControllerOption(controller),
        ))
        .with_children(|option| {
            option.spawn((Text::new(label), TextColor(Color::WHITE)));
        })
        .id()
}

/// Rebuilds the dropdown's options to show only input methods not already assigned
/// to a player on this client.
///
/// In Bevy 0.15+ each connected controller is an entity carrying a `Gamepad`
/// component (and a `Name` for its product name). The available list is "Keyboard"
/// plus every connected gamepad, minus any controller already taken by one of this
/// client's players (`client.players`). This runs every frame but only
/// despawns/respawns options when the available list differs from what is shown
/// (tracked via a `Local`), so it covers already-connected pads on entry as well as
/// later connects/disconnects and players joining, without any event plumbing.
///
/// It also revalidates the selection: if no input method is available the selection
/// is cleared and the header reads "No input methods left"; otherwise an invalid or
/// empty selection snaps to the first available option.
pub fn populate_controller_options(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    gamepads: Query<(Entity, Option<&Name>), With<Gamepad>>,
    panel: Query<Entity, With<ControllerDropdownPanel>>,
    options: Query<Entity, With<ControllerOption>>,
    mut selected: ResMut<SelectedController>,
    mut header_text: Query<&mut Text, With<ControllerDropdownHeaderText>>,
    mut shown: Local<Option<Vec<Controller>>>,
) {
    // Controllers already taken by a player on this client.
    let assigned: Vec<Controller> = client
        .client
        .read()
        .unwrap()
        .players
        .read()
        .unwrap()
        .iter()
        .map(|p| p.controller)
        .collect();

    // Build the available list: Keyboard first, then each connected gamepad,
    // skipping anything already assigned. Labels/numbering derive from the full
    // connected set so they stay stable per pad regardless of what's filtered out.
    let mut pads: Vec<_> = gamepads.iter().collect();
    pads.sort_by_key(|(e, _)| *e); // stable display order

    let mut available: Vec<(Controller, String)> = Vec::new();
    if !assigned.contains(&Controller::Keyboard) {
        available.push((Controller::Keyboard, controller_label(Controller::Keyboard)));
    }
    for (i, (entity, name)) in pads.iter().enumerate() {
        // The entity index uniquely identifies a connected controller, so two
        // pads with identical product names can still be told apart. It is
        // client-local and only used as a display/intent label.
        let controller = Controller::Gamepad(entity.index().index());
        if assigned.contains(&controller) {
            continue;
        }
        let label = match name {
            Some(n) => format!("{}: {} [#{}]", i + 1, n.as_str(), entity.index()),
            None => format!("Controller {} [#{}]", i + 1, entity.index()),
        };
        available.push((controller, label));
    }

    // Only rebuild when the available list actually changed.
    let signature: Vec<Controller> = available.iter().map(|(c, _)| *c).collect();
    if shown.as_ref() == Some(&signature) {
        return;
    }
    *shown = Some(signature);

    for option in &options {
        commands.entity(option).despawn();
    }
    let Ok(panel) = panel.single() else {
        return;
    };
    for (controller, label) in &available {
        let option = spawn_option_button(&mut commands, label.clone(), *controller);
        commands.entity(panel).add_child(option);
    }

    // Revalidate the selection against the available list and update the header.
    let still_valid = selected.0.map(|c| available.iter().any(|(ac, _)| *ac == c)).unwrap_or(false);
    if !still_valid {
        selected.0 = available.first().map(|(c, _)| *c);
    }
    if let Ok(mut text) = header_text.single_mut() {
        text.0 = match selected.0 {
            Some(c) => controller_label(c),
            None => "No input methods left".to_string(),
        };
    }
}

/// Toggles the dropdown panel open/closed when the header is clicked.
pub fn toggle_controller_dropdown(
    headers: Query<&Interaction, (Changed<Interaction>, With<ControllerDropdownHeader>)>,
    mut root: Query<&mut DropdownOpen, With<ControllerDropdownRoot>>,
    mut panel: Query<&mut Node, With<ControllerDropdownPanel>>,
) {
    for interaction in &headers {
        if *interaction == Interaction::Pressed {
            let Ok(mut open) = root.single_mut() else {
                return;
            };
            open.0 = !open.0;
            if let Ok(mut node) = panel.single_mut() {
                node.display = if open.0 { Display::Flex } else { Display::None };
            }
        }
    }
}

/// Applies a clicked option: stores the selection, updates the header text, and
/// collapses the panel.
pub fn handle_controller_option_click(
    options: Query<(&Interaction, &ControllerOption), Changed<Interaction>>,
    mut selected: ResMut<SelectedController>,
    mut header_text: Query<&mut Text, With<ControllerDropdownHeaderText>>,
    mut root: Query<&mut DropdownOpen, With<ControllerDropdownRoot>>,
    mut panel: Query<&mut Node, With<ControllerDropdownPanel>>,
) {
    for (interaction, option) in &options {
        if *interaction == Interaction::Pressed {
            selected.0 = Some(option.0);
            if let Ok(mut text) = header_text.single_mut() {
                text.0 = controller_label(option.0);
            }
            if let Ok(mut open) = root.single_mut() {
                open.0 = false;
            }
            if let Ok(mut node) = panel.single_mut() {
                node.display = Display::None;
            }
        }
    }
}

/// Emits a `JoinLobby` event with the typed name when the "Join" button is pressed.
pub fn handle_lobby_join_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<LobbyJoinButton>)>,
    name_field: Query<&InputField, With<LobbyNameField>>,
    selected: Res<SelectedController>,
    client: Res<GameClientWrapper>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            let Ok(field) = name_field.single() else {
                continue;
            };
            let name = field.value().trim();
            if name.is_empty() {
                continue;
            }
            let Some(controller) = selected.0 else {
                continue; // no input method available to join with
            };
            let client_guard = client.client.read().unwrap();
            let Some(client_id) = *client_guard.client_id.read().unwrap() else {
                continue; // not registered yet
            };
            if let Some(sender) = &client_guard.sender {
                sender.send(ClientEvent::JoinLobby { client_id, name: name.to_string(), controller }).ok();
            }
        }
    }
}

/// Emits a `GameStart` event when the "Start Game" button is pressed.
pub fn handle_lobby_start_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<LobbyStartButton>)>,
    client: Res<GameClientWrapper>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            let client_guard = client.client.read().unwrap();
            if let Some(sender) = &client_guard.sender {
                sender.send(ClientEvent::StartGame).ok();
            }
        }
    }
}

/// Dims the "Join" button while no input method is available, so it reads as
/// disabled. Gated on `SelectedController` change-detection, since availability
/// only changes when `populate_controller_options` re-points the selection.
pub fn update_join_button_state(
    selected: Res<SelectedController>,
    mut button: Query<&mut BackgroundColor, With<LobbyJoinButton>>,
) {
    if !selected.is_changed() {
        return;
    }
    if let Ok(mut bg) = button.single_mut() {
        bg.0 = if selected.0.is_some() {
            Color::srgb(0.2, 0.2, 0.25)
        } else {
            Color::srgb(0.1, 0.1, 0.1)
        };
    }
}

/// Tears down the lobby screen and its camera when leaving the lobby state.
pub fn cleanup_lobby(
    mut commands: Commands,
    root: Query<Entity, With<LobbyRoot>>,
    camera: Query<Entity, With<LobbyCamera>>,
) {
    for entity in &root {
        commands.entity(entity).despawn();
    }
    for entity in &camera {
        commands.entity(entity).despawn();
    }
}
