# kcc2d

A small, reusable **2D kinematic character controller** for [Bevy 0.19], built on [avian2d 0.7].

It is the controller behind [`run-2d`] and [`una_run`], and it was pulled out of the first of them
because the second one needed it too. Everything in it is tuned for a pixel platformer — the
defaults assume a character about eight world units tall — but every number is a field on
[`ControllerConfig`](src/lib.rs).

```rust
app.add_plugins(KccPlugin::default());          // runs in `FixedUpdate`

commands.spawn((
    KinematicController,
    Collider::rectangle(2.0, 7.0),
    ControllerConfig { half_extents: Vec2::new(1.0, 3.5), ..default() },
));
```

Write [`ControllerInput`](src/lib.rs) each frame, read [`ControllerState`](src/lib.rs) for
animation and game logic. That is the whole surface.

## What it does

- Acceleration-based movement with separate ground/air control and friction
- Gravity with a terminal velocity
- Ground detection (downward shape cast) with **coyote time** and a **jump buffer**
- **Variable jump height**: the jump is as high as the button is held for, up to the full arc —
  release early and gravity gets heavier for the rest of the rise, so a tap is a hop
- **Wall detection**, **wall slide** (clamped fall speed), and **wall jump** whose horizontal
  strength depends on input (into wall = small hop, neutral = medium, away = full leap)
- **Crouch-slide** (press crouch while moving fast): keeps momentum with low friction and **ducks
  the collider**, so you fit through low gaps a standing character can't
- **Crouch** (press crouch while slow, or automatically when a slide runs out of momentum): a
  ducked, controllable slow walk. You only stand back up when there's headroom *and* crouch is
  released, so ending a slide under a low ceiling drops into a crouch instead of getting stuck

## Rollback

The controller drives avian's `Position` rather than `Transform`, and that is the property that
makes it usable under rollback netcode: a snapshot restores `Position` and leaves the render
transform alone. A controller that read `Transform` would move away from the mispredicted value a
snapshot never restored, so the correction would be discarded before anything could act on it and
the character would rubber-band without ever converging. Nothing errors; it just feels like bad
netcode. `tests/rollback.rs` is what keeps that true.

`KccPlugin::new(schedule)` puts the controller in whatever schedule owns the simulation, because a
rollback simulation re-runs its schedule many times per frame and cannot use `FixedUpdate` at all:

```rust
app.add_plugins(KccPlugin::new(TickedSimulation));
// and, so the controller still runs before the physics step as it does under `FixedUpdate`:
app.configure_sets(TickedSimulation, KccSet.in_set(PhysicsSystems::First));
```

The controller reads `Res<Time>` for its timestep, so whatever installs the schedule is responsible
for installing a clock.

## Features

| Feature | What it adds |
|---|---|
| `debug` | `KccDebugPlugin`: gizmos for the collider, velocity and probe casts, plus a live text readout. `Tab` toggles it. |
| `serde` | `Serialize`/`Deserialize` on the components. Needed by any rollback network: `ControllerState`'s `coyote`, `jump_buffer`, `wall_lockout` and `prev_crouch` decide whether the next jump fires and whether a crouch press starts a slide, so a snapshot that leaves them out quietly resets the character's buffered input on every correction. |

## Trying it

```sh
# A/D move, Space jump/wall-jump, S crouch-slide, Tab toggles the debug overlay
cargo run --example playground --features debug

# Controller behaviour, and replay determinism under rollback
cargo test --all-features
```

[Bevy 0.19]: https://bevyengine.org
[avian2d 0.7]: https://github.com/avianphysics/avian
[`run-2d`]: https://github.com/Sigma-studios/run-2d
[`una_run`]: https://github.com/Sigma-studios/una_run
