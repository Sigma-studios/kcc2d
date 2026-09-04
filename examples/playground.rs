//! Standalone playground for the `kcc2d` controller.
//!
//! Controls: **A/D** or **←/→** to move, **Space** to jump (and wall-jump), **S/↓** to
//! crouch-slide. The debug overlay (gizmos + text readout) starts on here; press **Tab** to
//! toggle it.
//!
//! Run with: `cargo run -p kcc2d --example playground --features debug`

use avian2d::{math::*, prelude::*};
use bevy::{camera::ScalingMode, prelude::*};
use kcc2d::*;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(ImagePlugin::default_nearest()),
            PhysicsPlugins::default().with_length_unit(8.0),
            KccPlugin::default(),
        ))
        .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
        // This example showcases the debug overlay, so start with it on (Tab toggles it).
        .insert_resource(KccDebug(true))
        .add_systems(Startup, setup)
        .add_systems(Update, read_input)
        .run();
}

/// Marker for the controllable box.
#[derive(Component)]
struct Player;

fn setup(mut commands: Commands) {
    // Pixel-perfect-ish camera: show a small slice of the world so pixels stay chunky.
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 140.0,
            },
            ..OrthographicProjection::default_2d()
        }),
    ));

    // The controllable character: a 2x7 box, matching the game's player collider.
    commands.spawn((
        Player,
        KinematicController,
        Collider::rectangle(2.0, 7.0),
        ControllerConfig {
            half_extents: Vector::new(1.0, 3.5),
            ..default()
        },
        Sprite {
            color: Color::srgb(0.9, 0.6, 0.3),
            custom_size: Some(Vec2::new(2.0, 7.0)),
            ..default()
        },
        Transform::from_xyz(-90.0, -20.0, 0.0),
    ));

    // Floor (top surface at y = -34).
    spawn_solid(&mut commands, Vec2::new(0.0, -40.0), Vec2::new(300.0, 12.0));
    // A step.
    spawn_solid(&mut commands, Vec2::new(-55.0, -18.0), Vec2::new(48.0, 6.0));
    // Slide-only gate: underside 4px above the floor, so you must slide through it.
    spawn_solid(&mut commands, Vec2::new(0.0, -6.0), Vec2::new(24.0, 48.0));
    // A vertical shaft with walls on both sides for wall-jump testing.
    spawn_solid(&mut commands, Vec2::new(96.0, 6.0), Vec2::new(10.0, 90.0));
    spawn_solid(&mut commands, Vec2::new(60.0, 20.0), Vec2::new(10.0, 90.0));
}

fn spawn_solid(commands: &mut Commands, center: Vec2, size: Vec2) {
    commands.spawn((
        Sprite {
            color: Color::srgb(0.28, 0.30, 0.38),
            custom_size: Some(size),
            ..default()
        },
        Transform::from_translation(center.extend(-1.0)),
        RigidBody::Static,
        Collider::rectangle(size.x, size.y),
    ));
}

fn read_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut ControllerInput, With<Player>>,
) {
    let left = keyboard.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = keyboard.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]);

    for mut input in &mut query {
        input.move_x = (right as i8 - left as i8) as Scalar;
        // Latch the jump; the controller clears it once consumed. Never set it back to false.
        if keyboard.just_pressed(KeyCode::Space) {
            input.jump = true;
        }
        // The hold is what decides how high the jump goes.
        input.jump_held = keyboard.pressed(KeyCode::Space);
        input.crouch = keyboard.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]);
    }
}
