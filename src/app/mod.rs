pub mod screens;
pub mod common;

use std::sync::{Arc, RwLock};

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::app::screens::app_state::*;
use crate::app::common::text::*;
use crate::app::screens::game_menu::*;
use crate::app::screens::game_play::actions::*;
use crate::app::screens::game_play::animations::*;
use crate::app::screens::game_play::detect_events::*;
use crate::app::screens::game_play::phases::end::*;
use crate::app::screens::game_play::state::*;
use crate::app::screens::game_play::world::*;
use crate::app::screens::join_game::*;
use crate::app::screens::lobby::*;
use crate::{client::GameClient};

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
        .init_resource::<SelectedColor>()
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
            (
                focus_input_field,
                update_input,
                handle_paste_button,
                handle_join_online_submit_button,
                handle_join_local_server_button,
            )
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
                handle_color_swatch_click,
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
                start_jump,
                start_swing,
                detect_player_movement,
                apply_dead_collision_layers,
                animate_death,
                start_throwing,
                start_pullback,
                stop_pullback,
                start_throw_animation,
                animate_throwing_action,
                release_throw,
            )
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            PhysicsSchedule,
            (
                stop_pulling_boomerang.before(pull_boomerang),
                pull_boomerang.before(release_boomerang),
                release_boomerang.before(wind_up_boomerang),
                wind_up_boomerang.before(jump_player),
                jump_player.before(swing_boomerang),
                swing_boomerang.before(move_player),
                move_player.before(detect_throw_strikes),
                detect_throw_strikes.before(detect_parries),
                detect_parries.before(animate_swing),
                animate_swing.before(detect_swing_strikes),
                detect_swing_strikes.before(PhysicsStepSystems::First),
                record_tick_state.after(PhysicsStepSystems::Last),
            )
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            FixedUpdate,
            (drain_server_events).run_if(in_state(AppState::Playing))
        )
        .add_systems(OnEnter(AppState::RoundEndAnimation), start_round_end_animation)
        .add_systems(Update, animate_round_end.run_if(in_state(AppState::RoundEndAnimation)))
        .add_systems(OnEnter(AppState::RoundEnded), setup_round_ended)
        .add_systems(
            Update,
            (handle_continue_button, wait_for_next_round).run_if(in_state(AppState::RoundEnded)),
        )
        .add_systems(OnExit(AppState::RoundEnded), cleanup_round_ended)
        .run();
}
