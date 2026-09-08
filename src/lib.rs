//! `kcc2d` — a small, reusable **2D kinematic character controller** for Bevy, built on
//! [`avian2d`].
//!
//! The controller drives a [`RigidBody::Kinematic`] body with avian's move-and-slide
//! algorithm and layers platformer feel on top of it:
//!
//! - Acceleration-based horizontal movement with separate ground/air control and friction.
//! - Gravity with a terminal velocity.
//! - Ground detection via a downward shape cast, with **coyote time** and a **jump buffer**.
//! - **Variable jump height**: the jump is as high as the button is held for, up to the full arc.
//! - **Wall detection** via sideways shape casts, enabling **wall slide** and **wall jump**.
//! - **Crouch-slide**: holding crouch while moving fast preserves momentum with low friction.
//!
//! The public surface is intentionally data-oriented: spawn a [`KinematicController`], write to
//! its [`ControllerInput`] each frame, and read its [`ControllerState`] for animation/logic.
//!
//! With the `debug` feature enabled, [`KccDebugPlugin`] draws the controller internals
//! (collider, velocity, probe casts, and a live text readout).

use avian2d::{math::*, prelude::*};
use bevy::ecs::schedule::{InternedScheduleLabel, ScheduleLabel};
use bevy::prelude::*;

/// Shorthand for the `serde` feature's derive, so each type carries one attribute instead of two.
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "debug")]
mod debug;
#[cfg(feature = "debug")]
pub use debug::{KccDebug, KccDebugPlugin, TOGGLE_KEY};

/// System set containing every controller system, in whichever schedule [`KccPlugin`] was given.
/// Order your own input-writing systems before this set and animation/read systems after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KccSet;

/// Plugin that registers the controller components and systems.
///
/// Defaults to [`FixedUpdate`], which is the classic fixed-timestep placement. Construct it with
/// [`KccPlugin::new`] to run the controller in another schedule — a rollback simulation owns its
/// own, re-runs it many times per frame while replaying, and cannot use `FixedUpdate` at all:
///
/// ```text
/// app.add_plugins(KccPlugin::new(TickedSimulation));
/// // and, so the controller still runs before the physics step as it does under FixedUpdate:
/// app.configure_sets(TickedSimulation, KccSet.in_set(PhysicsSystems::First));
/// ```
///
/// The controller reads `Res<Time>` for its timestep, so whatever installs the schedule is
/// responsible for installing a clock: `bevy_ticked`'s `run_tick_schedule` does exactly this,
/// rebuilding the clock from the tick counter so a replayed tick reproduces its original delta.
pub struct KccPlugin {
    schedule: InternedScheduleLabel,
}

impl KccPlugin {
    /// Run the controller in `schedule` instead of [`FixedUpdate`].
    pub fn new(schedule: impl ScheduleLabel) -> Self {
        Self {
            schedule: schedule.intern(),
        }
    }
}

impl Default for KccPlugin {
    fn default() -> Self {
        Self::new(FixedUpdate)
    }
}

impl Plugin for KccPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ControllerConfig>()
            .register_type::<ControllerInput>()
            .register_type::<ControllerState>()
            .register_type::<Side>();

        app.add_systems(
            self.schedule,
            (
                detect_contacts,
                tick_timers,
                update_pose,
                apply_gravity,
                horizontal_movement,
                handle_jump,
                do_move_and_slide,
                sync_state,
            )
                .chain()
                .in_set(KccSet),
        );

        #[cfg(feature = "debug")]
        app.add_plugins(KccDebugPlugin);
    }
}

/// Which side of the character something is on (a wall) or which way it faces.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Side {
    Left,
    #[default]
    Right,
}

impl Side {
    /// `-1.0` for [`Side::Left`], `+1.0` for [`Side::Right`].
    pub fn sign(self) -> Scalar {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }
}

/// Marker for an entity driven by the kinematic character controller.
///
/// Requires the avian components needed for manual kinematic movement, plus the controller's
/// own config/input/state. You still need to add a [`Collider`] (its shape is what actually
/// collides) and set [`ControllerConfig::half_extents`] to match that collider.
#[derive(Component)]
#[require(
    RigidBody::Kinematic,
    CustomPositionIntegration,
    // We move the body ourselves, so speculative collision impulses would only fight us.
    SpeculativeMargin(0.0),
    LinearVelocity,
    ControllerConfig,
    ControllerInput,
    ControllerState,
)]
pub struct KinematicController;

/// Tunable parameters for a [`KinematicController`]. All distances are in world units
/// (for the pixel game, 1 unit = 1 pixel).
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControllerConfig {
    /// Half-extents of the character's collider box. Used to size the detection probes.
    pub half_extents: Vector,
    /// Which [`CollisionLayers`] memberships count as solid for the controller's ground/wall/
    /// move-and-slide probes. Defaults to every layer; narrow it to make the character ignore
    /// props (pickups, decorations) that are on their own layer.
    pub mask: LayerMask,
    /// Maximum horizontal speed reachable from input.
    pub max_speed: Scalar,
    /// Horizontal acceleration toward the input target speed.
    pub accel: Scalar,
    /// Deceleration applied when grounded with no horizontal input.
    pub ground_friction: Scalar,
    /// Multiplier applied to `accel`/`ground_friction` while airborne (0..=1).
    pub air_control: Scalar,
    /// Downward gravitational acceleration (positive magnitude).
    pub gravity: Scalar,
    /// Maximum downward speed reachable from gravity.
    pub terminal_velocity: Scalar,
    /// Upward speed applied on a ground jump. Holding jump for the whole rise gives this arc;
    /// letting go early ends it sooner — see `jump_release_gravity_scale`.
    pub jump_speed: Scalar,
    /// Gravity multiplier while rising with jump *not* held. This is what makes jump height
    /// follow how long the button is down: a tap is a hop, a hold is the full `jump_speed` arc,
    /// and everything in between is in between. `1.0` turns the variable height off.
    pub jump_release_gravity_scale: Scalar,
    /// Grace period after leaving the ground during which a jump is still allowed.
    pub coyote_time: Scalar,
    /// How long a jump press is remembered while airborne, so it fires on landing.
    pub jump_buffer_time: Scalar,
    /// Velocity applied on a wall jump: `x` is the *away-from-wall* horizontal speed (used when
    /// holding away from the wall or with no input), `y` is upward speed.
    pub wall_jump_speed: Vector,
    /// Horizontal wall-jump scale when holding *into* the wall (small hop up the wall).
    pub wall_jump_into_scale: Scalar,
    /// Horizontal wall-jump scale with *no* horizontal input (medium arc off the wall).
    pub wall_jump_neutral_scale: Scalar,
    /// Duration after a wall jump during which horizontal input is ignored, so the character
    /// actually leaves the wall.
    pub wall_jump_lockout: Scalar,
    /// Maximum downward speed while wall-sliding (positive magnitude).
    pub wall_slide_speed: Scalar,
    /// Friction applied while crouch-sliding (kept low to preserve momentum).
    pub slide_friction: Scalar,
    /// Minimum horizontal speed required to enter a crouch-slide.
    pub slide_min_speed: Scalar,
    /// Collider half-height while sliding. The character ducks to this height with its feet
    /// planted, so it fits through low gaps a standing character can't.
    pub slide_half_height: Scalar,
    /// Maximum horizontal speed while crouch-walking (a slide drops to a crouch once it decays
    /// below this speed).
    pub crouch_speed: Scalar,
    /// Maximum angle (radians) from "up" for a surface to count as ground rather than a wall.
    pub max_ground_angle: Scalar,
    /// How far the probes cast when looking for ground/walls.
    pub probe_distance: Scalar,
    /// Amount the probes are shrunk relative to the collider so they don't self-catch.
    pub skin: Scalar,
    /// Maximum move-and-slide iterations per fixed step.
    pub max_slide_iterations: usize,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            half_extents: Vector::new(1.0, 3.5),
            mask: LayerMask::ALL,
            max_speed: 60.0,
            accel: 600.0,
            ground_friction: 700.0,
            air_control: 0.55,
            gravity: 520.0,
            terminal_velocity: 220.0,
            jump_speed: 150.0,
            jump_release_gravity_scale: 2.5,
            coyote_time: 0.09,
            jump_buffer_time: 0.1,
            wall_jump_speed: Vector::new(95.0, 145.0),
            wall_jump_into_scale: 0.25,
            wall_jump_neutral_scale: 0.6,
            wall_jump_lockout: 0.18,
            wall_slide_speed: 40.0,
            slide_friction: 35.0,
            slide_min_speed: 18.0,
            slide_half_height: 1.5,
            crouch_speed: 22.0,
            max_ground_angle: PI / 5.0,
            probe_distance: 1.5,
            skin: 0.35,
            max_slide_iterations: 8,
        }
    }
}

/// Per-frame intent written by the game (in `Update`/`PreUpdate`) and consumed by the
/// controller in [`FixedUpdate`].
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControllerInput {
    /// Desired horizontal movement in `-1.0..=1.0`.
    pub move_x: Scalar,
    /// Jump request latch. The game should only ever set this to `true` (on the press edge,
    /// e.g. `just_pressed`) and never clear it: the controller consumes it — buffering the
    /// jump and resetting this flag — on the next fixed step. This makes jumps robust to high
    /// frame rates, where a fixed step may not run on the single frame a key is pressed.
    pub jump: bool,
    /// Whether the jump button is *held*, as opposed to the press edge above. Read while rising:
    /// letting go cuts the jump short, so height follows the length of the press.
    pub jump_held: bool,
    /// Whether crouch is held. Pressing it while running fast on the ground starts a slide.
    pub crouch: bool,
}

/// Read-only-ish snapshot of the controller, produced each fixed step. Read this for
/// animation and gameplay logic.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControllerState {
    /// Current velocity (mirrors the body's [`LinearVelocity`]).
    pub velocity: Vector,
    /// Whether the character is standing on ground this step.
    pub grounded: bool,
    /// Normal of the ground under the character (valid when `grounded`).
    pub ground_normal: Vector,
    /// Which side a wall was detected on, if any.
    pub wall: Option<Side>,
    /// Whether the character is clinging to a wall: airborne and pushing into it, in either
    /// vertical direction. Drives the wall pose/animation (shown while going up *and* down).
    pub wall_clinging: bool,
    /// Whether the character is actively sliding *down* a wall (clinging while descending). This
    /// is what clamps the fall speed to [`ControllerConfig::wall_slide_speed`].
    pub wall_sliding: bool,
    /// Whether the character is in a momentum crouch-slide (ducked, no input control).
    pub sliding: bool,
    /// Whether the character is in a controllable crouch (ducked, can walk slowly). Entered by
    /// holding crouch while slow, or when a slide runs out of momentum, and held until there's
    /// headroom to stand *and* crouch is released.
    pub crouching: bool,
    /// The direction the character is facing.
    pub facing: Side,
    /// Remaining coyote time.
    pub coyote: Scalar,
    /// Remaining buffered-jump time.
    pub jump_buffer: Scalar,
    /// Remaining wall-jump input lockout.
    pub wall_lockout: Scalar,
    /// Internal: previous-frame crouch state, used to edge-trigger the slide.
    prev_crouch: bool,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            velocity: Vector::ZERO,
            grounded: false,
            ground_normal: Vector::Y,
            wall: None,
            wall_clinging: false,
            wall_sliding: false,
            sliding: false,
            crouching: false,
            facing: Side::Right,
            coyote: 0.0,
            jump_buffer: 0.0,
            wall_lockout: 0.0,
            prev_crouch: false,
        }
    }
}

impl ControllerState {
    /// Whether the collider is ducked (either momentum-sliding or crouching).
    pub fn ducked(&self) -> bool {
        self.sliding || self.crouching
    }
}

/// The body's simulation position.
///
/// Avian's [`Position`] is the authority when the entity has one, and [`Transform`] is only a
/// render-space copy of it — so a controller that reads and writes `Transform` instead is reading
/// a value that a rollback network never restores. Snapshots carry `Position`; the stale
/// `Transform` left over from a mispredicted tick is what the controller would move away from,
/// and the correction is lost before anything can act on it.
///
/// Falling back to `Transform` keeps the controller usable without avian's `Position` at all,
/// which is what it did before.
fn body_position(position: Option<&Position>, transform: &Transform) -> Vector {
    match position {
        Some(position) => position.0,
        None => transform.translation.xy().adjust_precision(),
    }
}

/// Writes the body's simulation position back, mirroring [`body_position`].
///
/// Writing `Position` unconditionally — every tick, moving or not — is deliberate: avian's
/// `transform_to_position` copies `Transform` onto `Position` only for bodies whose `Position`
/// was *not* touched since the last physics tick, so a controller that always writes it can never
/// have a render-space value copied back over the simulation.
fn set_body_position(position: Option<Mut<Position>>, transform: &mut Transform, value: Vector) {
    match position {
        Some(mut position) => position.0 = value,
        None => {
            transform.translation = value.f32().extend(transform.translation.z);
        }
    }
}

/// Moves `current` toward `target` by at most `max_delta`.
fn approach(current: Scalar, target: Scalar, max_delta: Scalar) -> Scalar {
    if current < target {
        (current + max_delta).min(target)
    } else {
        (current - max_delta).max(target)
    }
}

impl ControllerConfig {
    /// Half-extents of the collider box for the current pose: the character ducks to
    /// [`ControllerConfig::slide_half_height`] while ducked (sliding or crouching), and stands
    /// full height otherwise.
    pub fn pose_half_extents(&self, ducked: bool) -> Vector {
        let half_y = if ducked {
            self.slide_half_height
        } else {
            self.half_extents.y
        };
        Vector::new(self.half_extents.x, half_y)
    }

    /// Vertical offset from the transform (the standing body centre) to the *current* collision
    /// box centre. Zero when standing; negative (lower) while ducked, so the feet stay planted
    /// without moving the transform — keeping the sprite anchored in place.
    pub fn pose_offset(&self, ducked: bool) -> Scalar {
        if ducked {
            self.slide_half_height - self.half_extents.y
        } else {
            0.0
        }
    }
}

/// Shape-casts to update grounded/ground-normal and wall side in [`ControllerState`].
fn detect_contacts(
    mut query: Query<
        (
            Entity,
            &ControllerConfig,
            Option<&Position>,
            &Transform,
            &mut ControllerState,
        ),
        With<KinematicController>,
    >,
    spatial_query: SpatialQuery,
) {
    for (entity, cfg, position, transform, mut state) in &mut query {
        // Probe from the current (possibly ducked) box centre; the body position is the standing
        // centre, so shift down by the pose offset while ducked.
        let origin =
            body_position(position, transform) + Vector::Y * cfg.pose_offset(state.ducked());
        let filter = SpatialQueryFilter::from_excluded_entities([entity]).with_mask(cfg.mask);
        let config = ShapeCastConfig::from_max_distance(cfg.probe_distance);

        // Build probes from the current pose so detection matches the (possibly ducked) body.
        let half = cfg.pose_half_extents(state.ducked());
        let skin = cfg.skin;
        // Narrower than the body so it slips past walls when probing down.
        let vertical = Collider::rectangle((half.x - skin).max(0.05) * 2.0, half.y * 2.0);
        // Shorter than the body so it slips past floor/ceiling when probing sideways.
        let horizontal = Collider::rectangle(half.x * 2.0, (half.y - skin).max(0.05) * 2.0);

        // Ground: cast the narrow box straight down.
        let ground =
            spatial_query.cast_shape(&vertical, origin, 0.0, Dir2::NEG_Y, &config, &filter);
        state.grounded =
            ground.is_some_and(|hit| Vector::Y.angle_to(hit.normal1).abs() <= cfg.max_ground_angle);
        state.ground_normal = ground
            .filter(|_| state.grounded)
            .map_or(Vector::Y, |hit| hit.normal1);

        // Walls: cast the short box left and right. A hit only counts if its normal is
        // clearly horizontal (so we don't mistake a floor edge for a wall).
        let is_wall = |hit: ShapeHitData| hit.normal1.x.abs() > 0.5;
        let left = spatial_query
            .cast_shape(&horizontal, origin, 0.0, Dir2::NEG_X, &config, &filter)
            .filter(|h| is_wall(*h));
        let right = spatial_query
            .cast_shape(&horizontal, origin, 0.0, Dir2::X, &config, &filter)
            .filter(|h| is_wall(*h));
        state.wall = match (left, right) {
            (Some(_), _) => Some(Side::Left),
            (_, Some(_)) => Some(Side::Right),
            _ => None,
        };
    }
}

/// Advances coyote time, jump buffering, and wall-jump lockout.
fn tick_timers(
    time: Res<Time>,
    mut query: Query<(
        &ControllerConfig,
        &mut ControllerInput,
        &mut ControllerState,
    )>,
) {
    let dt = time.delta_secs().adjust_precision();
    for (cfg, mut input, mut state) in &mut query {
        if state.grounded {
            state.coyote = cfg.coyote_time;
        } else {
            state.coyote = (state.coyote - dt).max(0.0);
        }

        // Consume the jump latch into the buffer so a press is never lost between fixed steps.
        if input.jump {
            state.jump_buffer = cfg.jump_buffer_time;
            input.jump = false;
        } else {
            state.jump_buffer = (state.jump_buffer - dt).max(0.0);
        }

        state.wall_lockout = (state.wall_lockout - dt).max(0.0);
    }
}

/// Resolves the ducked pose (momentum slide vs. controllable crouch vs. standing) and ducks the
/// collider (feet stay planted). Key rules:
///
/// - Pressing crouch while running fast starts a **slide** (momentum, no input control).
/// - A slide that decays below `crouch_speed`, or crouching from a standstill, becomes a
///   **crouch** (ducked, walk-controllable).
/// - You only **stand up** when the full-height body fits (headroom) *and* crouch is released,
///   so ending a slide under a low ceiling drops into a crouch instead of getting stuck.
fn update_pose(
    spatial_query: SpatialQuery,
    mut query: Query<
        (
            Entity,
            &ControllerConfig,
            &ControllerInput,
            &LinearVelocity,
            &mut ControllerState,
            Option<&Position>,
            &Transform,
        ),
        With<KinematicController>,
    >,
) {
    for (entity, cfg, input, vel, mut state, position, transform) in &mut query {
        // Edge-detect crouch: a slide only starts on the press, not while merely held.
        let crouch_pressed = input.crouch && !state.prev_crouch;
        state.prev_crouch = input.crouch;

        let grounded = state.grounded;

        // Is there room to stand up (full-height box at the standing centre)? Slightly shrunk so
        // resting on the ground / brushing a wall doesn't count as blocked.
        let can_stand = {
            let stand_box = Collider::rectangle(
                (cfg.half_extents.x - 0.1).max(0.05) * 2.0,
                (cfg.half_extents.y - 0.1).max(0.05) * 2.0,
            );
            let centre = body_position(position, transform);
            let filter = SpatialQueryFilter::from_excluded_entities([entity]).with_mask(cfg.mask);
            spatial_query
                .shape_intersections(&stand_box, centre, 0.0, &filter)
                .is_empty()
        };

        // A slide ends when it leaves the ground, crouch is released, or momentum runs out. It
        // ends into a crouch if crouch is still held — or if there is no headroom to stand in,
        // whatever the keys say. Standing is a 7px box where a 3px one was: with a ceiling in the
        // way the body comes up already overlapping it, and move-and-slide resolves that overlap
        // the only way it can, by shoving the body down — 3.5px in one frame, into the floor.
        // Letting go of crouch under a gate, or sliding off a ledge under one, both did this.
        if state.sliding && (!grounded || !input.crouch || vel.x.abs() <= cfg.crouch_speed) {
            let keep_ducked = (grounded && input.crouch) || !can_stand;
            state.sliding = false;
            state.crouching = keep_ducked;
        }

        if !state.sliding {
            if grounded && crouch_pressed && vel.x.abs() > cfg.slide_min_speed && !state.crouching {
                // Fresh, fast crouch press from standing -> slide.
                state.sliding = true;
                state.crouching = false;
            } else if grounded && input.crouch {
                // Holding crouch on the ground -> crouch (idle or walking).
                state.crouching = true;
            } else if state.crouching && can_stand {
                // Crouch is released (or we're airborne) and there's room: stand up.
                state.crouching = false;
            }
            // Otherwise stay crouched — e.g. stuck under a low ceiling until it clears.
        }
    }
}

/// Applies gravity, cuts a released jump short, and clamps fall speed while wall-sliding.
fn apply_gravity(
    time: Res<Time>,
    mut query: Query<(
        &ControllerConfig,
        &ControllerInput,
        &mut ControllerState,
        &mut LinearVelocity,
    )>,
) {
    let dt = time.delta_secs().adjust_precision();
    for (cfg, input, mut state, mut vel) in &mut query {
        if !state.grounded {
            // Rising with the button up: heavier gravity, so the arc ends where the press did.
            // Holding it gives plain gravity and the full `jump_speed` height.
            let gravity = if vel.y > 0.0 && !input.jump_held {
                cfg.gravity * cfg.jump_release_gravity_scale
            } else {
                cfg.gravity
            };
            vel.y = (vel.y - gravity * dt).max(-cfg.terminal_velocity);
        } else if vel.y < 0.0 {
            // Small downward bias keeps the ground probe engaged on flat ground.
            vel.y = vel.y.max(-cfg.gravity * dt);
        }

        // Wall cling: airborne and pushing into a wall, in either vertical direction — this is
        // the wall pose. The wall *slide* additionally requires descending, and is what clamps
        // the fall speed (clamping upward motion would feel wrong).
        let pushing_into_wall = state
            .wall
            .is_some_and(|side| input.move_x.signum() == side.sign() && input.move_x != 0.0);
        state.wall_clinging = !state.grounded && pushing_into_wall && state.wall_lockout <= 0.0;
        state.wall_sliding = state.wall_clinging && vel.y < 0.0;
        if state.wall_sliding {
            vel.y = vel.y.max(-cfg.wall_slide_speed);
        }
    }
}

/// Horizontal acceleration, friction, and slide/crouch handling.
fn horizontal_movement(
    time: Res<Time>,
    mut query: Query<(
        &ControllerConfig,
        &ControllerInput,
        &mut ControllerState,
        &mut LinearVelocity,
    )>,
) {
    let dt = time.delta_secs().adjust_precision();
    for (cfg, input, mut state, mut vel) in &mut query {
        // While the wall-jump lockout is active, let momentum carry the character off the wall.
        if state.wall_lockout > 0.0 {
            continue;
        }

        let control = if state.grounded { 1.0 } else { cfg.air_control };
        // Crouching caps the speed; sliding and standing use the full run speed.
        let max_speed = if state.crouching {
            cfg.crouch_speed
        } else {
            cfg.max_speed
        };

        if state.sliding {
            // Preserve momentum: only gentle friction, no input acceleration.
            vel.x = approach(vel.x, 0.0, cfg.slide_friction * dt);
        } else if input.move_x != 0.0 {
            let target = input.move_x.clamp(-1.0, 1.0) * max_speed;
            vel.x = approach(vel.x, target, cfg.accel * control * dt);
            state.facing = if input.move_x > 0.0 {
                Side::Right
            } else {
                Side::Left
            };
        } else {
            vel.x = approach(vel.x, 0.0, cfg.ground_friction * control * dt);
        }
    }
}

/// Ground jumps (with coyote/buffer) and wall jumps.
fn handle_jump(
    mut query: Query<(
        &ControllerConfig,
        &ControllerInput,
        &mut ControllerState,
        &mut LinearVelocity,
    )>,
) {
    for (cfg, input, mut state, mut vel) in &mut query {
        if state.jump_buffer <= 0.0 {
            continue;
        }

        if state.grounded || state.coyote > 0.0 {
            vel.y = cfg.jump_speed;
            state.jump_buffer = 0.0;
            state.coyote = 0.0;
        } else if let Some(side) = state.wall {
            let away = -side.sign(); // horizontal direction away from the wall

            // Horizontal strength depends on input relative to the wall: holding away gives the
            // full leap, no input a medium arc, and holding into the wall only a small hop
            // (useful for climbing the wall with repeated jumps).
            let scale = if input.move_x == 0.0 {
                cfg.wall_jump_neutral_scale
            } else if input.move_x.signum() == away {
                1.0
            } else {
                cfg.wall_jump_into_scale
            };

            vel.x = away * cfg.wall_jump_speed.x * scale;
            vel.y = cfg.wall_jump_speed.y;
            state.facing = if side == Side::Left {
                Side::Right
            } else {
                Side::Left
            };
            state.wall_lockout = cfg.wall_jump_lockout;
            state.jump_buffer = 0.0;
        }
    }
}

/// One controller's move, gathered before [`MoveAndSlide`] is borrowed.
///
/// The split exists because [`MoveAndSlide`] reads every collider's [`Position`], which conflicts
/// with the `&mut Position` the writeback needs. Gathering into a reused buffer, computing, then
/// writing back through a [`ParamSet`] keeps the three accesses disjoint without allocating per
/// frame.
struct MoveRequest {
    entity: Entity,
    origin: Vector,
    offset: Vector,
    half: Vector,
    velocity: Vector,
    mask: LayerMask,
    max_ground_angle: Scalar,
    iterations: usize,
}

/// The outcome of one [`MoveRequest`], applied in the writeback pass.
struct MoveResult {
    entity: Entity,
    position: Vector,
    /// Velocity correction along `up`, or `None` when nothing vertical was hit.
    vertical_correction: Option<Scalar>,
}

/// Moves each controller with avian's move-and-slide and corrects velocity on ground/ceiling
/// contact so it doesn't accumulate into surfaces.
fn do_move_and_slide(
    mut params: ParamSet<(
        Query<
            (
                Entity,
                &ControllerConfig,
                &ControllerState,
                &LinearVelocity,
                Option<&Position>,
                &Transform,
            ),
            With<KinematicController>,
        >,
        MoveAndSlide,
        Query<
            (Option<&mut Position>, &mut Transform, &mut LinearVelocity),
            With<KinematicController>,
        >,
    )>,
    time: Res<Time>,
    mut requests: Local<Vec<MoveRequest>>,
    mut results: Local<Vec<MoveResult>>,
) {
    let delta = time.delta();
    let up = Vector::Y;

    // 1. Gather. Use the current pose so a ducked body fits through low gaps: the body position
    //    is the standing centre, so move-and-slide runs on a box shifted down by the pose offset
    //    and the result is shifted back up.
    requests.clear();
    for (entity, cfg, state, vel, position, transform) in params.p0().iter() {
        let ducked = state.ducked();
        requests.push(MoveRequest {
            entity,
            origin: body_position(position, transform),
            offset: Vector::Y * cfg.pose_offset(ducked),
            half: cfg.pose_half_extents(ducked),
            velocity: vel.0,
            mask: cfg.mask,
            max_ground_angle: cfg.max_ground_angle,
            iterations: cfg.max_slide_iterations,
        });
    }

    // 2. Solve.
    results.clear();
    {
        let move_and_slide = params.p1();
        for request in requests.iter() {
            let mut hit_ground_or_ceiling = false;
            let config = MoveAndSlideConfig {
                move_and_slide_iterations: request.iterations,
                ..default()
            };

            let MoveAndSlideOutput {
                position,
                projected_velocity,
            } = move_and_slide.move_and_slide(
                &Collider::rectangle(request.half.x * 2.0, request.half.y * 2.0),
                request.origin + request.offset,
                0.0,
                request.velocity,
                delta,
                &config,
                &SpatialQueryFilter::from_excluded_entities([request.entity])
                    .with_mask(request.mask),
                |hit| {
                    let angle = up.angle_to(hit.normal.adjust_precision()).abs();
                    if angle <= request.max_ground_angle || angle >= PI - request.max_ground_angle {
                        hit_ground_or_ceiling = true;
                    }
                    MoveAndSlideHitResponse::Accept
                },
            );

            results.push(MoveResult {
                entity: request.entity,
                position: position - request.offset,
                // Cancel accumulated velocity into the surface we hit vertically.
                vertical_correction: hit_ground_or_ceiling
                    .then(|| projected_velocity.dot(up) - request.velocity.dot(up)),
            });
        }
    }

    // 3. Write back.
    let mut bodies = params.p2();
    for result in results.iter() {
        let Ok((position, mut transform, mut vel)) = bodies.get_mut(result.entity) else {
            continue;
        };
        set_body_position(position, &mut transform, result.position);
        if let Some(correction) = result.vertical_correction {
            vel.0 += correction * up;
        }
    }
}

/// Mirrors the body velocity into [`ControllerState`] for reading.
fn sync_state(mut query: Query<(&mut ControllerState, &LinearVelocity)>) {
    for (mut state, vel) in &mut query {
        state.velocity = vel.0;
    }
}
