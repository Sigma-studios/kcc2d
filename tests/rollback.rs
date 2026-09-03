//! Rollback-safety tests.
//!
//! A rollback network rewinds the world to an authoritative state and re-runs the simulation
//! from there, many times a frame. That only works if the controller is a *pure function* of the
//! state a snapshot actually carries — `Position`, `LinearVelocity` and [`ControllerState`] — and
//! of the input for each replayed tick.
//!
//! The failure this guards against is specific and silent: a controller that reads `Transform`
//! rather than avian's `Position` moves away from the *mispredicted* value a snapshot never
//! restored, so the correction is discarded before anything can act on it and the character
//! rubber-bands without ever converging. Nothing errors; it just feels like bad netcode.
//!
//! So the replay here deliberately restores only the three simulation components and leaves
//! `Transform` holding a wrong value.

use std::time::Duration;

use avian2d::math::Vector;
use avian2d::prelude::*;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use kcc2d::*;

/// 64 Hz, matching the tick rate the networking stack runs at.
const DT: f64 = 1.0 / 64.0;
const TOTAL_TICKS: u64 = 120;
/// The tick the "snapshot" is taken at and replayed from.
const ROLLBACK_TICK: u64 = 60;

/// Stands in for the simulation schedule a rollback crate would own.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Sim;

/// The three components a snapshot carries for a controller-driven body.
#[derive(Clone)]
struct SimState {
    position: Vector,
    velocity: Vector,
    controller: ControllerState,
}

fn sim_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        // Avian's diagnostics systems need the resources that this plugin registers.
        bevy::diagnostic::DiagnosticsPlugin,
        PhysicsPlugins::new(Sim).with_length_unit(8.0),
        KccPlugin::new(Sim),
    ));
    // The controller runs before the physics step, exactly as it does under `FixedUpdate`.
    app.configure_sets(Sim, KccSet.in_set(PhysicsSystems::First));
    app.insert_resource(Gravity(Vec2::NEG_Y * ControllerConfig::default().gravity));
    app
}

fn solid(app: &mut App, centre: Vec2, size: Vec2) {
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::rectangle(size.x, size.y),
        Position(centre),
    ));
}

fn spawn_controller(app: &mut App, at: Vec2) -> Entity {
    app.world_mut()
        .spawn((
            KinematicController,
            Collider::rectangle(2.0, 7.0),
            ControllerConfig::default(),
            Position(at),
        ))
        .id()
}

/// A fixed input script: run right, jump, turn around, crouch-slide, jump again.
///
/// `jump` is written as the press edge for that tick and re-written on every replay of it, which
/// is what makes the controller's jump latch reproducible under rollback.
fn scripted_input(tick: u64) -> ControllerInput {
    ControllerInput {
        move_x: if tick < 40 {
            1.0
        } else if tick < 70 {
            -1.0
        } else {
            1.0
        },
        jump: matches!(tick, 25 | 55 | 95),
        crouch: (75..90).contains(&tick),
    }
}

fn step(app: &mut App, entity: Entity, tick: u64) {
    app.world_mut()
        .entity_mut(entity)
        .insert(scripted_input(tick));

    // Whatever drives the schedule owns the clock; here that is the test.
    let mut clock = Time::default();
    clock.advance_by(Duration::from_secs_f64(DT));
    *app.world_mut().resource_mut::<Time>() = clock;

    app.world_mut().run_schedule(Sim);
}

fn read_state(app: &mut App, entity: Entity) -> SimState {
    let entity = app.world().entity(entity);
    SimState {
        position: entity
            .get::<Position>()
            .expect("controller has a Position")
            .0,
        velocity: entity
            .get::<LinearVelocity>()
            .expect("controller has a LinearVelocity")
            .0,
        controller: entity
            .get::<ControllerState>()
            .expect("controller has a ControllerState")
            .clone(),
    }
}

/// Runs the script from tick 1 to `TOTAL_TICKS`, returning every tick's state.
fn run_reference() -> (App, Entity, Vec<SimState>) {
    let mut app = sim_app();
    solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(420.0, 12.0));
    solid(&mut app, Vec2::new(60.0, 24.0), Vec2::new(6.0, 60.0));
    let player = spawn_controller(&mut app, Vec2::new(-40.0, 20.0));
    app.finish();
    app.cleanup();

    let mut history = Vec::new();
    for tick in 1..=TOTAL_TICKS {
        step(&mut app, player, tick);
        history.push(read_state(&mut app, player));
    }
    (app, player, history)
}

#[test]
fn replaying_from_a_restored_snapshot_reproduces_the_original_path() {
    let (mut app, player, reference) = run_reference();

    // The script has to actually exercise the controller, or this test passes vacuously.
    assert!(
        reference
            .iter()
            .any(|state| state.controller.grounded && state.velocity.x.abs() > 10.0),
        "the input script never got the character running on the ground"
    );

    // Restore the world to what a snapshot for `ROLLBACK_TICK` would carry — and nothing else.
    let restored = reference[(ROLLBACK_TICK - 1) as usize].clone();
    {
        let mut entity = app.world_mut().entity_mut(player);
        entity.insert((
            Position(restored.position),
            LinearVelocity(restored.velocity),
            restored.controller.clone(),
        ));
        // Left deliberately wrong. `Transform` is a render-space copy that no snapshot restores,
        // so a controller that reads it would replay from here and never converge.
        entity.insert(Transform::from_xyz(999.0, 999.0, 0.0));
    }

    for tick in (ROLLBACK_TICK + 1)..=TOTAL_TICKS {
        step(&mut app, player, tick);
        let replayed = read_state(&mut app, player);
        let expected = &reference[(tick - 1) as usize];
        assert_eq!(
            replayed.position, expected.position,
            "tick {tick} replayed to a different position"
        );
        assert_eq!(
            replayed.velocity, expected.velocity,
            "tick {tick} replayed to a different velocity"
        );
        assert_eq!(
            replayed.controller.grounded, expected.controller.grounded,
            "tick {tick} replayed to a different grounded state"
        );
        assert_eq!(
            replayed.controller.jump_buffer, expected.controller.jump_buffer,
            "tick {tick} replayed with a different buffered jump"
        );
    }
}

#[test]
fn the_controller_drives_avian_position() {
    let mut app = sim_app();
    solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(420.0, 12.0));
    let player = spawn_controller(&mut app, Vec2::new(0.0, 40.0));
    app.finish();
    app.cleanup();

    let start = read_state(&mut app, player).position;
    for tick in 1..=10 {
        step(&mut app, player, tick);
    }
    let end = read_state(&mut app, player);

    assert!(
        end.position.y < start.y,
        "the controller must fall by writing Position, not only Transform"
    );

    // And avian's writeback must have carried it into render space.
    let transform = app.world().entity(player).get::<Transform>().unwrap();
    assert!(
        (transform.translation.y - end.position.y).abs() < 1e-4,
        "Transform ({}) did not follow Position ({})",
        transform.translation.y,
        end.position.y
    );
}

#[test]
fn a_body_without_avian_position_still_moves_via_transform() {
    // The fallback path: the controller stays usable for anyone not replicating `Position`.
    let mut app = sim_app();
    solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(420.0, 12.0));
    let player = app
        .world_mut()
        .spawn((
            KinematicController,
            Collider::rectangle(2.0, 7.0),
            ControllerConfig::default(),
            Transform::from_xyz(0.0, 40.0, 0.0),
        ))
        .id();
    app.finish();
    app.cleanup();

    for tick in 1..=10 {
        step(&mut app, player, tick);
    }

    let y = app
        .world()
        .entity(player)
        .get::<Transform>()
        .unwrap()
        .translation
        .y;
    assert!(y < 40.0, "the character did not fall (y = {y})");
}
