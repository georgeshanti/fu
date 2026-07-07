pub mod screens;
pub mod common;

use std::sync::{Arc, RwLock};

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::app::screens::app_state::*;
use crate::app::common::text::*;
use crate::app::screens::game_menu::*;
use crate::app::screens::game_play::*;
use crate::app::screens::join_game::*;
use crate::app::screens::lobby::*;
use crate::{client::GameClient, server::{ClientEvent, ServerEvent}};

#[derive(Resource)]
pub struct GameClientWrapper {
    pub client: Arc<RwLock<GameClient>>,
}

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .init_state::<AppState>()
        .init_resource::<SelectedController>()
        .add_systems(OnEnter(AppState::Menu), setup_menu)
        .add_systems(
            Update,
            (handle_join_game_button, handle_create_game_button, update_server_status_text)
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(OnExit(AppState::Menu), cleanup_menu)
        .add_systems(OnEnter(AppState::JoinGame), setup_join_screen)
        .add_systems(
            Update,
            (focus_input_field, update_input, handle_join_online_submit_button, handle_join_local_server_button)
                .run_if(in_state(AppState::JoinGame)),
        )
        .add_systems(OnExit(AppState::JoinGame), cleanup_join_screen)
        .add_systems(OnEnter(AppState::Lobby), setup_lobby)
        .add_systems(
            Update,
            (
                focus_input_field,
                update_input,
                update_lobby,
                handle_lobby_join_button,
                handle_lobby_start_button,
                populate_controller_options,
                toggle_controller_dropdown,
                handle_controller_option_click,
                update_join_button_state,
            )
                .run_if(in_state(AppState::Lobby)),
        )
        .add_systems(OnExit(AppState::Lobby), cleanup_lobby)
        .add_systems(OnEnter(AppState::SpawningPlayers), setup_game_play)
        .add_systems(
            Update,
            (tick_countdown, wait_for_start).run_if(in_state(AppState::SpawningPlayers)),
        )
        .add_systems(
            Update,
            (
                move_player,
                start_swing,
                animate_swing,
                tick_swing_cooldown,
                detect_strikes,
                apply_dead_collision_layers,
                animate_death,
                record_tick_state.after(move_player).after(start_swing).after(detect_strikes),
            )
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            FixedUpdate,
            (drain_server_events).run_if(in_state(AppState::Playing))
        )
        .run();
}
