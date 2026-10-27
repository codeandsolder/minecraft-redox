# Vanilla sparse-interaction graph spike

This note records the experimental path from vanilla 1.7.10 bytecode to the target interaction graph. It is deliberately separate from the high-level architecture: this is evidence and implementation detail, not a compatibility commitment to the old tick loop.

## Corpus sampled

The existing ForgeGradle 1.7.10 artifacts were enough to start without adding another archaeology input. `DumpReachable` was run with depth 5 against the SRG Minecraft jar.

| Root | Reachable methods | Classes | Jimple statements |
| --- | ---: | ---: | ---: |
| `TileEntityFurnace.updateEntity` | 154 | 42 | 4,458 |
| `TileEntityBrewingStand.updateEntity` | 117 | 37 | 3,603 |
| `TileEntityHopper.updateEntity` | 78 | 22 | 2,082 |
| `BlockCrops.updateTick` | 100 | 27 | 3,026 |
| `BlockRedstoneDiode.updateTick` | 125 | 28 | 3,390 |
| `BlockRedstoneTorch.updateTick` | 132 | 31 | 3,548 |

The dumps live under `/home/jan/research/minecraft-redox-202610/vanilla-1.7.10/`; they are research artifacts and are not committed.

## Structural temporal recovery

`minecraft-redox-java` now exposes `JimpleMethod::temporal_summary()`, and the CLI exposes it as `jimple-temporal`. The analysis follows Jimple local definitions but intentionally stops at heap fields: a comparison after a field write should describe the current field, not recursively rewrite it into the pre-write expression.

Without knowing any Minecraft class or field names, the analysis recovers:

- constant-delta instance/static field updates such as countdowns and countups;
- constant field writes such as timer resets;
- comparisons of fields against constant thresholds;
- receiver provenance so similarly named fields on different objects are not conflated.

On the vanilla sample this immediately finds three tick-polling counters:

- furnace burn time: decrement by 1, with zero/nonzero boundary tests;
- furnace cook time: increment by 1, reset to 0, with the 200 boundary test;
- brewing time: decrement by 1, reset to 0 or 400;
- hopper cooldown: decrement by 1 (its 0/8 resets are hidden behind a parameterized setter, which is the next interprocedural effect-summary target).

By contrast, crop, repeater, and torch update handlers do not contain local polling counters. That is useful negative evidence rather than a failure: the repeater/torch are already invoked by the world scheduled-update mechanism, while crops receive stochastic random-tick opportunities.

## Execution model validated in Quiescence

Quiescence commit `7a45dc9` adds first-class sparse reactions:

- dynamic read dependencies;
- generation-stamped forecast invalidation when any dependency changes;
- delayed causal transitions;
- atomic batches of state writes;
- a sparse reverse index for reaction dependencies so reaction-free node graphs do not carry an extra adjacency set per node;
- next-event introspection for proactive simulation.

A furnace-shaped example stores burn/cook deadlines rather than replaying 20 Hz polling. With three inputs and one coal-equivalent fuel, 80 seconds of simulated time reaches the same coarse terminal state using four causal queue events versus 1,600 legacy tick callbacks. This is a runtime-shape demonstration, not yet a claim of full vanilla furnace parity.

## Target lowering shape

The graph should encode sparse causal semantics, not historical polling frequency.

1. **Polling counter -> deadline/epoch.** A monotonic counter whose only meaningful observations are thresholds becomes an absolute deadline or epoch plus on-demand views. GUI progress is derived from `(deadline, now)` rather than creating one graph wakeup per visible tick.
2. **Scheduled callback -> causal graph event.** Vanilla redstone scheduling is already close to the target model. Preserve delay, ordering/priority, deduplication, and reevaluate guards when the event fires.
3. **Random opportunity -> stochastic event source.** Crop random ticks should enter through an explicit scheduler/RNG event source. Do not invent a deterministic per-crop polling node.
4. **Interaction -> transaction.** Hopper transfers and machine completions need an atomic resource/state transaction. Reads determine dependencies; writes become visible together at the causal boundary.
5. **Presentation/effects -> outputs.** Dirty notifications, sounds, particles, and client progress are effects/views of semantic state changes, not reasons to poll the simulation.
6. **Dense work stays specialized.** Block breaking, bulk spatial edits, rendering, collision/physics inner loops, and similar dense operations should keep specialized representations. Their sparse boundary effects can still feed the graph.

## Interprocedural structural effects

The next generic step is now implemented locally: each method exposes an ordered direct effect stream of symbolic field writes and invocations. Program-level composition substitutes `this` and parameters through exact in-program calls, preserves the call path for each possible write, cuts recursive call-graph cycles explicitly, and refuses ambiguous dynamic dispatch. Static/special calls are exact; virtual calls are composed only when the receiver is structurally known to be exactly the declared owner (for example a same-class call on the entry `this`).

On the real vanilla Hopper `updateEntity` reachable dump this recovers the cooldown field in three forms without Hopper-specific names:

- direct decrement: `cooldown = cooldown - 1`;
- one-hop helper effect: `setCooldown(0)` -> cooldown write `0`;
- two-hop successful-transfer effect: `tryTransfer()` -> `setCooldown(8)` -> cooldown write `8`.

These are reported as **possible structural effects**, not unconditional writes. CFG guards still need to be attached before they become executable graph reactions. Missing methods and unresolved dynamic dispatch remain explicit in the summary rather than being guessed.

## CFG guards and predicate recovery

Structural effects now carry branch guards recovered from the Jimple CFG. For each `if`, the false/true successor regions are separated using immediate post-dominators; an effect inherits a branch predicate only when it is reachable from exactly one arm before reconvergence. Guard expressions resolve locals through CFG **reaching definitions**, not textual proximity. If multiple definitions can reach a use, the local remains symbolic instead of choosing an arbitrary assignment.

Program-level composition substitutes `this`/arguments through both effects and their guards, simplifies boolean comparisons, inlines exact boolean-materialization helpers, normalizes negation into guard polarity, and deduplicates equivalent predicates inherited through nested calls. This is still structural: guards are conjunctions of necessary control predicates, not a claim that every opaque call inside them is already lowered.

On the real vanilla Hopper `updateEntity` path this now yields:

- the cooldown decrement guarded by non-null world, server-side execution, and cooldown `> 0`;
- `setCooldown(0)` guarded by non-null world, server-side execution, and concrete `cooldown <= 0` (the legacy `func_145888_j()` helper is gone);
- the successful-transfer `setCooldown(8)` path inherits the same `cooldown <= 0` predicate and lowers another static helper to its metadata bit test;
- the final transfer-success local `l1` remains symbolic because several branch-dependent definitions reach it. That is the correct next frontier: guarded/piecewise value reconstruction rather than unsafe nearest-definition substitution.

Furnace recovery also benefits immediately: burn-time decrement is structurally guarded by burn time `> 0`, while cook-time increment carries the server-side, input/fuel presence, burn-active, and can-smelt predicates.

## Missing pieces exposed by vanilla

The current graph runtime proves the scheduling model but intentionally still has prototype numeric state. Exact Minecraft lowering needs:

- typed/canonical state values or stable handles for item stacks, block states, inventories, entities, and other structured resources;
- interprocedural effect summaries with parameter substitution, so a call like `setCooldown(8)` recovers as a concrete write even when the callee assigns a parameter to the field;
- semantic lowering for the world scheduled-update queue, including priority and duplicate/coalescing behavior;
- explicit stochastic event/RNG semantics for random ticks and random outcomes;
- bounded temporal-history/window primitives for behavior such as redstone-torch burnout;
- transaction primitives for inventory/resource movement across graph-owned objects.

The important result of this spike is that none of these require reintroducing a global tick loop. They are additions to the causal graph interface.
