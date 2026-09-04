//! Headless behavioural tests for the controller. These drive avian's physics with a fixed
//! manual clock (no renderer, no window) and assert the character falls, grounds, runs, and
//! jumps as expected.

use std::time::Duration;

use avian2d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use kcc2d::*;

const DT: f64 = 1.0 / 60.0;

/// Builds a headless app with physics + the controller (no geometry yet).
fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        // Avian's diagnostics systems need the resources that this plugin registers.
        bevy::diagnostic::DiagnosticsPlugin,
        PhysicsPlugins::default().with_length_unit(8.0),
        KccPlugin::default(),
    ));
    // Advance time by a fixed amount per `update()` so the sim is deterministic.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        DT,
    )));
    app
}

/// `app.update()` doesn't run plugin `finish()`/`cleanup()` (only `app.run()` does), and avian
/// inserts some of its resources there. Do it manually for the headless test.
fn finalize(app: &mut App) {
    app.finish();
    app.cleanup();
}

fn spawn_player_at(app: &mut App, pos: Vec2) -> Entity {
    app.world_mut()
        .spawn((
            KinematicController,
            Collider::rectangle(2.0, 7.0),
            ControllerConfig {
                half_extents: Vec2::new(1.0, 3.5),
                ..default()
            },
            Transform::from_xyz(pos.x, pos.y, 0.0),
        ))
        .id()
}

fn spawn_solid(app: &mut App, center: Vec2, size: Vec2) {
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::rectangle(size.x, size.y),
        Transform::from_xyz(center.x, center.y, 0.0),
    ));
}

/// A flat-floor scenario: floor top at y = 0, character high above it.
fn test_app() -> (App, Entity) {
    let mut app = base_app();
    spawn_solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(400.0, 12.0));
    let player = spawn_player_at(&mut app, Vec2::new(0.0, 40.0));
    finalize(&mut app);
    (app, player)
}

fn step(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn state(app: &App, e: Entity) -> ControllerState {
    app.world().get::<ControllerState>(e).unwrap().clone()
}

fn set_input(app: &mut App, e: Entity, move_x: f32, jump: bool, crouch: bool) {
    let mut input = app.world_mut().get_mut::<ControllerInput>(e).unwrap();
    input.move_x = move_x;
    input.jump = jump;
    input.crouch = crouch;
}

fn y(app: &App, e: Entity) -> f32 {
    app.world().get::<Transform>(e).unwrap().translation.y
}

fn x(app: &App, e: Entity) -> f32 {
    app.world().get::<Transform>(e).unwrap().translation.x
}

#[test]
fn falls_and_lands_on_ground() {
    let (mut app, player) = test_app();
    let start_y = y(&app, player);

    // Let gravity do its thing.
    step(&mut app, 240);

    assert!(y(&app, player) < start_y, "character should have fallen");
    let s = state(&app, player);
    assert!(s.grounded, "character should be grounded after landing");
    // Rests with its 3.5 half-height on the floor top at y = 0.
    let resting = y(&app, player);
    assert!(
        (2.5..=4.5).contains(&resting),
        "resting y should be ~3.5, was {resting}"
    );
}

#[test]
fn runs_horizontally_on_input() {
    let (mut app, player) = test_app();
    step(&mut app, 240); // land first
    let start_x = x(&app, player);

    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 60);

    let moved = x(&app, player) - start_x;
    assert!(
        moved > 5.0,
        "character should have moved right, delta was {moved}"
    );
    assert_eq!(state(&app, player).facing, Side::Right);
}

#[test]
fn jumps_off_the_ground() {
    let (mut app, player) = test_app();
    step(&mut app, 240); // land
    let resting = y(&app, player);
    assert!(state(&app, player).grounded);

    // One-frame jump press, then release and watch it rise.
    set_input(&mut app, player, 0.0, true, false);
    app.update();
    set_input(&mut app, player, 0.0, false, false);

    let mut max_y = y(&app, player);
    for _ in 0..40 {
        app.update();
        max_y = max_y.max(y(&app, player));
    }

    assert!(
        max_y > resting + 5.0,
        "jump should raise the character; peak {max_y} vs resting {resting}"
    );
}

fn jump_flag(app: &App, e: Entity) -> bool {
    app.world().get::<ControllerInput>(e).unwrap().jump
}

#[test]
fn jump_latch_is_consumed_once() {
    let (mut app, player) = test_app();
    step(&mut app, 240); // land
    let resting = y(&app, player);

    // Latch a jump and then leave the flag alone, as the game does.
    set_input(&mut app, player, 0.0, true, false);
    app.update();

    // The controller must have consumed (cleared) the latch on the first fixed step.
    assert!(
        !jump_flag(&app, player),
        "controller should clear the jump latch"
    );

    // It rises...
    let mut max_y = y(&app, player);
    for _ in 0..40 {
        app.update();
        max_y = max_y.max(y(&app, player));
    }
    assert!(max_y > resting + 5.0, "latched jump should fire once");

    // ...and does not spontaneously jump again once landed (latch was cleared, not re-set).
    step(&mut app, 240);
    assert!(state(&app, player).grounded);
    let landed = y(&app, player);
    step(&mut app, 40);
    assert!(
        (y(&app, player) - landed).abs() < 1.0,
        "should stay put; the latch must not re-trigger"
    );
}

#[test]
fn coyote_time_allows_a_late_jump() {
    let mut app = base_app();
    // A short platform to walk off the right edge of (top at y = 0, right edge at x = 20).
    spawn_solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(40.0, 12.0));
    let player = spawn_player_at(&mut app, Vec2::new(-10.0, 20.0));
    finalize(&mut app);
    step(&mut app, 120); // land on the platform
    assert!(state(&app, player).grounded);

    // Walk off the right edge into the void.
    set_input(&mut app, player, 1.0, false, false);
    let mut left_ground = false;
    for _ in 0..120 {
        app.update();
        if !state(&app, player).grounded {
            left_ground = true;
            break;
        }
    }
    assert!(left_ground, "should have walked off the ledge");
    assert!(
        state(&app, player).coyote > 0.0,
        "coyote timer should be active just after leaving the ground"
    );

    // Jump within the coyote window even though we're airborne — it should lift us.
    let y0 = y(&app, player);
    set_input(&mut app, player, 1.0, true, false);
    app.update();
    set_input(&mut app, player, 1.0, false, false);
    let mut peak = y(&app, player);
    for _ in 0..25 {
        app.update();
        peak = peak.max(y(&app, player));
    }
    assert!(
        peak > y0 + 3.0,
        "coyote jump should lift the character (peak {peak}, y0 {y0})"
    );
}

#[test]
fn jump_buffer_fires_on_landing() {
    let mut app = base_app();
    spawn_solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(400.0, 12.0)); // floor, top at 0
    // Generous buffer so a press well before landing still counts.
    let player = app
        .world_mut()
        .spawn((
            KinematicController,
            Collider::rectangle(2.0, 7.0),
            ControllerConfig {
                half_extents: Vec2::new(1.0, 3.5),
                jump_buffer_time: 0.5,
                ..default()
            },
            Transform::from_xyz(0.0, 40.0, 0.0),
        ))
        .id();
    finalize(&mut app);

    // Press jump once while high in the air; the latch buffers it for landing.
    set_input(&mut app, player, 0.0, true, false);
    assert!(!state(&app, player).grounded, "should start airborne");

    // Fall until the first ground contact.
    for _ in 0..300 {
        app.update();
        if state(&app, player).grounded {
            break;
        }
    }
    assert!(state(&app, player).grounded, "should have landed");
    let contact_y = y(&app, player);

    // The buffered jump should launch us straight back up on landing.
    let mut peak = contact_y;
    for _ in 0..30 {
        app.update();
        peak = peak.max(y(&app, player));
    }
    assert!(
        peak > contact_y + 3.0,
        "buffered jump should fire on landing (peak {peak}, contact {contact_y})"
    );
}

#[test]
fn slide_decays_into_crouch_without_flicker() {
    let (mut app, player) = test_app();
    step(&mut app, 240); // land

    // Build up speed to the right.
    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 90);
    assert!(
        state(&app, player).velocity.x > 20.0,
        "should be running fast"
    );

    // Press crouch (edge) while fast -> slide starts.
    set_input(&mut app, player, 1.0, false, true);
    app.update();
    let s = state(&app, player);
    assert!(
        s.sliding && !s.crouching,
        "crouch press while fast should start a slide"
    );

    // Keep crouch + right held while speed decays: it must stay ducked the whole time (never
    // flicker up to standing/run) and eventually settle into a controllable crouch.
    let mut became_crouch = false;
    for _ in 0..180 {
        app.update();
        let s = state(&app, player);
        assert!(
            s.ducked(),
            "must stay ducked while crouch is held on the ground"
        );
        became_crouch |= s.crouching;
    }
    assert!(
        became_crouch,
        "a slide that runs out of momentum should become a crouch"
    );
    let s = state(&app, player);
    assert!(
        s.crouching && !s.sliding,
        "should end up crouching, not sliding"
    );

    // Releasing crouch (with headroom) stands back up.
    set_input(&mut app, player, 0.0, false, false);
    step(&mut app, 3);
    assert!(
        !state(&app, player).ducked(),
        "releasing crouch should stand up"
    );
}

#[test]
fn sliding_keeps_the_transform_height() {
    // The body ducks by offsetting its collision box, not by moving the transform, so the
    // sprite (anchored to the transform) doesn't sink into the ground while sliding.
    let (mut app, player) = test_app();
    step(&mut app, 240); // land

    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 30);
    let y_running = y(&app, player);

    set_input(&mut app, player, 1.0, false, true);
    app.update();
    assert!(state(&app, player).sliding, "should be sliding");

    assert!(
        (y(&app, player) - y_running).abs() < 0.6,
        "transform height must not change when ducking (was {y_running}, now {})",
        y(&app, player)
    );
}

#[test]
fn crouch_from_standstill_then_walk() {
    let (mut app, player) = test_app();
    step(&mut app, 240); // land, at rest

    // Press crouch while stopped -> crouch (not slide).
    set_input(&mut app, player, 0.0, false, true);
    app.update();
    let s = state(&app, player);
    assert!(
        s.crouching && !s.sliding,
        "crouch from a standstill should crouch, not slide"
    );

    // Move while holding crouch -> crouch-walk, capped at the (slow) crouch speed.
    set_input(&mut app, player, 1.0, false, true);
    step(&mut app, 120);
    let s = state(&app, player);
    assert!(s.crouching, "should still be crouching while down is held");
    assert!(
        s.velocity.x > 5.0 && s.velocity.x < 25.0,
        "crouch-walk should move but stay slow (was {})",
        s.velocity.x
    );

    // Release crouch (with headroom) -> stand up.
    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 3);
    assert!(
        !state(&app, player).crouching,
        "releasing crouch should stand up"
    );
}

#[test]
fn stays_crouched_under_a_low_ceiling_until_clear() {
    // Ending a slide (or crouching) under a low ceiling must not get stuck standing: the body
    // stays crouched until there's headroom *and* crouch is released.
    let (mut app, player) = tunnel_app(); // ceiling at x in [20, 36], underside at y = 4
    step(&mut app, 120); // land

    // Crouch-walk to the right until we're underneath the ceiling.
    set_input(&mut app, player, 1.0, false, true);
    let mut under = false;
    for _ in 0..400 {
        app.update();
        if x(&app, player) > 24.0 {
            under = true;
            break;
        }
    }
    assert!(under, "should have crouch-walked under the ceiling");
    assert!(
        state(&app, player).crouching,
        "should be crouching under the ceiling"
    );

    // Release crouch while still under the ceiling: can't stand (no headroom), stays crouched.
    set_input(&mut app, player, 0.0, false, false);
    step(&mut app, 10);
    assert!(
        state(&app, player).crouching,
        "must stay crouched with no headroom, even with crouch released"
    );

    // Move out from under the ceiling (still forced-crouched), then it stands once clear.
    set_input(&mut app, player, 1.0, false, false);
    for _ in 0..400 {
        app.update();
        if !state(&app, player).crouching {
            break;
        }
    }
    assert!(
        !state(&app, player).crouching && x(&app, player) > 36.0,
        "should stand up once past the ceiling (x = {})",
        x(&app, player)
    );
}

/// A character wall-sliding against a wall on its right (holding "right" into it).
fn wall_sliding_app() -> (App, Entity) {
    let mut app = base_app();
    // A tall wall whose left face sits at x = 1.5, just right of the character.
    spawn_solid(&mut app, Vec2::new(4.5, 20.0), Vec2::new(6.0, 80.0));
    let player = spawn_player_at(&mut app, Vec2::new(0.0, 30.0));
    finalize(&mut app);

    // Fall while holding "right" so we press into the wall.
    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 40);
    (app, player)
}

#[test]
fn wall_slides_then_wall_jumps() {
    let (mut app, player) = wall_sliding_app();

    let s = state(&app, player);
    assert_eq!(
        s.wall,
        Some(Side::Right),
        "wall should be detected on the right"
    );
    assert!(s.wall_sliding, "character should be wall-sliding");
    // Fall speed is clamped to `wall_slide_speed` (default 40) while wall-sliding.
    assert!(
        (-41.0..=0.0).contains(&s.velocity.y),
        "fall speed should be clamped to wall_slide_speed while wall-sliding (was {})",
        s.velocity.y
    );

    // Wall jump holding *away* from the wall (left): full leap up and to the left.
    let x_before = x(&app, player);
    set_input(&mut app, player, -1.0, true, false);
    app.update();
    set_input(&mut app, player, 0.0, false, false);
    step(&mut app, 12);

    let s = state(&app, player);
    assert!(
        x(&app, player) < x_before,
        "wall jump should push the character away from the wall (left)"
    );
    assert_eq!(
        s.facing,
        Side::Left,
        "should face away from the wall after wall jump"
    );
}

#[test]
fn wall_clings_while_ascending_but_only_slides_while_descending() {
    let (mut app, player) = wall_sliding_app();
    let s = state(&app, player);
    assert!(
        s.wall_clinging && s.wall_sliding,
        "should cling and slide while descending"
    );

    // Force upward motion while still pressing into the wall.
    app.world_mut()
        .get_mut::<LinearVelocity>(player)
        .unwrap()
        .0
        .y = 120.0;
    set_input(&mut app, player, 1.0, false, false); // keep pushing into the (right) wall
    app.update();

    let s = state(&app, player);
    assert!(s.velocity.y > 0.0, "should be moving up");
    assert!(
        s.wall_clinging,
        "wall pose should show while ascending against the wall"
    );
    assert!(
        !s.wall_sliding,
        "the fall-speed clamp should not apply while ascending"
    );
}

#[test]
fn wall_jump_horizontal_scales_with_input() {
    // Launch horizontal speed right after a wall jump, for a given input at the jump moment.
    // The wall is on the right, so "away" is negative (left).
    let launch_vx = |move_x: f32| {
        let (mut app, player) = wall_sliding_app();
        set_input(&mut app, player, move_x, true, false);
        app.update();
        state(&app, player).velocity.x
    };

    let away = launch_vx(-1.0); // holding away from the wall -> full
    let neutral = launch_vx(0.0); // no input -> medium
    let into = launch_vx(1.0); // holding into the wall -> small

    // All push away (leftwards / negative), strongest when holding away, weakest into the wall.
    assert!(
        away < neutral && neutral < into && into < 0.0,
        "expected away < neutral < into < 0, got away={away}, neutral={neutral}, into={into}"
    );
}

/// A flat floor with a low ceiling forming a tunnel (its underside 4px above the floor) that
/// only a ducked, sliding character can pass through. The character starts to the left of it.
fn tunnel_app() -> (App, Entity) {
    let mut app = base_app();
    spawn_solid(&mut app, Vec2::new(0.0, -6.0), Vec2::new(400.0, 12.0)); // floor, top at 0
    // Ceiling block spanning x in [20, 36], underside at y = 4.
    spawn_solid(&mut app, Vec2::new(28.0, 24.0), Vec2::new(16.0, 40.0));
    let player = spawn_player_at(&mut app, Vec2::new(0.0, 20.0));
    finalize(&mut app);
    (app, player)
}

#[test]
fn standing_run_is_blocked_by_low_ceiling() {
    let (mut app, player) = tunnel_app();
    step(&mut app, 120); // land

    // Run right without sliding: the full-height body is stopped by the low ceiling.
    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 200);

    assert!(
        x(&app, player) < 20.0,
        "a standing character must not get through the low tunnel (x = {})",
        x(&app, player)
    );
}

#[test]
fn sliding_passes_under_low_ceiling() {
    let (mut app, player) = tunnel_app();
    step(&mut app, 120); // land

    // Build up speed while still clear of the tunnel...
    set_input(&mut app, player, 1.0, false, false);
    step(&mut app, 8);
    assert!(
        state(&app, player).velocity.x > 40.0,
        "should be moving fast before the tunnel"
    );

    // ...then slide (crouch press edge) and glide through the ducked-only gap.
    set_input(&mut app, player, 1.0, false, true);
    app.update();
    assert!(
        state(&app, player).sliding,
        "crouch press while fast should start a slide"
    );
    step(&mut app, 150);

    assert!(
        x(&app, player) > 40.0,
        "a sliding character should pass under the low tunnel (x = {})",
        x(&app, player)
    );
}
