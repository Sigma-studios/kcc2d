//! Debug visualisation for the controller: gizmos for the collider/velocity/probes and an
//! on-screen text readout of the first controller's [`ControllerState`].
//!
//! Enabled by the `debug` feature. Toggle at runtime with the [`KccDebug`] resource.

use avian2d::math::*;
use bevy::prelude::*;

use crate::{ControllerConfig, ControllerState, KinematicController, Side};

/// Key that toggles the debug overlay on/off.
pub const TOGGLE_KEY: KeyCode = KeyCode::Tab;

/// Runtime toggle for the debug overlay. Off by default; press [`TOGGLE_KEY`] to toggle.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct KccDebug(pub bool);

/// Draws controller gizmos and maintains the text readout.
pub struct KccDebugPlugin;

impl Plugin for KccDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KccDebug>()
            .add_systems(Startup, spawn_readout)
            .add_systems(
                Update,
                (
                    // Both of these need something a headless app does not have: keyboard input
                    // needs `InputPlugin`, gizmos need a renderer. A consumer running the
                    // controller in a simulation test has neither, and a missing resource is a
                    // panic rather than a skipped system.
                    toggle_debug.run_if(resource_exists::<ButtonInput<KeyCode>>),
                    draw_gizmos.run_if(resource_exists::<GizmoConfigStore>),
                    update_readout,
                ),
            );
    }
}

/// Toggles the overlay when [`TOGGLE_KEY`] is pressed.
fn toggle_debug(keyboard: Res<ButtonInput<KeyCode>>, mut debug: ResMut<KccDebug>) {
    if keyboard.just_pressed(TOGGLE_KEY) {
        debug.0 = !debug.0;
    }
}

const GROUNDED: Color = Color::srgb(0.35, 0.9, 0.4);
const AIRBORNE: Color = Color::srgb(0.95, 0.85, 0.3);
const WALL_SLIDE: Color = Color::srgb(0.3, 0.85, 0.95);
const PROBE: Color = Color::srgb(0.9, 0.3, 0.5);
const VEL: Color = Color::srgb(0.95, 0.5, 0.2);

fn draw_gizmos(
    debug: Res<KccDebug>,
    mut gizmos: Gizmos,
    query: Query<(&Transform, &ControllerConfig, &ControllerState), With<KinematicController>>,
) {
    if !debug.0 {
        return;
    }

    for (transform, cfg, state) in &query {
        // The collision box sits at the standing centre plus the pose offset (ducked = lower).
        let pos = transform.translation.xy() + Vec2::Y * cfg.pose_offset(state.ducked());
        let half = cfg.pose_half_extents(state.ducked());
        let size = (half * 2.0).f32();

        let color = if state.wall_clinging {
            WALL_SLIDE
        } else if state.grounded {
            GROUNDED
        } else {
            AIRBORNE
        };

        // Collider outline.
        gizmos.rect_2d(Isometry2d::from_translation(pos), size, color);

        // Velocity vector (scaled down so it reads at pixel scale).
        gizmos.arrow_2d(pos, pos + state.velocity.f32() * 0.15, VEL);

        // Ground probe (down) and wall probes (sides), highlighted when in contact.
        let he = half.f32();
        let d = cfg.probe_distance;
        let down_color = if state.grounded { GROUNDED } else { PROBE };
        gizmos.line_2d(
            pos + Vec2::new(0.0, -he.y),
            pos + Vec2::new(0.0, -he.y - d),
            down_color,
        );

        let left_color = if state.wall == Some(Side::Left) {
            WALL_SLIDE
        } else {
            PROBE
        };
        gizmos.line_2d(
            pos + Vec2::new(-he.x, 0.0),
            pos + Vec2::new(-he.x - d, 0.0),
            left_color,
        );
        let right_color = if state.wall == Some(Side::Right) {
            WALL_SLIDE
        } else {
            PROBE
        };
        gizmos.line_2d(
            pos + Vec2::new(he.x, 0.0),
            pos + Vec2::new(he.x + d, 0.0),
            right_color,
        );
    }
}

/// Marker for the debug text node.
#[derive(Component)]
struct DebugReadout;

fn spawn_readout(mut commands: Commands) {
    commands.spawn((
        DebugReadout,
        Text::new("kcc2d"),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb(0.9, 0.9, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}

fn update_readout(
    debug: Res<KccDebug>,
    controllers: Query<&ControllerState, With<KinematicController>>,
    mut readout: Query<(&mut Text, &mut Visibility), With<DebugReadout>>,
) {
    let Ok((mut text, mut visibility)) = readout.single_mut() else {
        return;
    };

    if !debug.0 {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;

    let Some(state) = controllers.iter().next() else {
        return;
    };

    let wall = match state.wall {
        Some(Side::Left) => "left",
        Some(Side::Right) => "right",
        None => "-",
    };

    text.0 = format!(
        "vel   {:>7.1}, {:>7.1}\n\
         grounded  {}\n\
         wall      {}\n\
         wallcling {}\n\
         wallslide {}\n\
         sliding   {}\n\
         crouching {}\n\
         facing    {:?}\n\
         coyote    {:.2}  buffer {:.2}  lockout {:.2}",
        state.velocity.x,
        state.velocity.y,
        state.grounded,
        wall,
        state.wall_clinging,
        state.wall_sliding,
        state.sliding,
        state.crouching,
        state.facing,
        state.coyote,
        state.jump_buffer,
        state.wall_lockout,
    );
}
