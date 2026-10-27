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
- hopper cooldown: decrement by 1; interprocedural effect composition now also recovers helper-mediated resets to `0` and `8`.

By contrast, crop, repeater, and torch update handlers do not contain local polling counters. That is useful negative evidence rather than a failure: the repeater/torch are already invoked by the world scheduled-update mechanism, while crops receive stochastic random-tick opportunities.

## Execution model validated in Quiescence

Quiescence commit `7a45dc9` adds first-class sparse reactions; `7a7d2c6` adds exact canonical atom/handle state alongside quantized numeric state:

- dynamic read dependencies;
- generation-stamped forecast invalidation when any dependency changes;
- delayed causal transitions;
- atomic batches of state writes;
- a sparse reverse index for reaction dependencies so reaction-free node graphs do not carry an extra adjacency set per node;
- next-event introspection for proactive simulation;
- exact `u64` canonical handles for immutable structured snapshots such as inventories/block states, with the same equality cutoff and dependency invalidation as numeric nodes;
- atomic mixed numeric + atom write batches.

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

These are reported as **possible structural effects**, not unconditional writes. CFG guards and guarded piecewise local values are now attached before composition; missing methods and unresolved dynamic dispatch remain explicit rather than being guessed.

## CFG guards and predicate recovery

Structural effects now carry branch guards recovered from the Jimple CFG. Guard recovery uses post-dominator control dependence plus ordinary dominance, so an effect inherits the branch decisions that actually control its execution in the same iteration. This replaced an earlier arm-reachability approximation after the redstone-torch loop exposed a subtle bug: a backedge could make a branch occurring *after* an effect appear able to reach that effect on the next iteration, incorrectly attaching a future predicate as a current guard. A synthetic loop regression now requires nested same-iteration guards to survive while rejecting later-iteration leakage. Guard expressions resolve locals through CFG **reaching definitions**, not textual proximity. If multiple definitions can reach a use, the local remains symbolic instead of choosing an arbitrary assignment.

Program-level composition substitutes `this`/arguments through both effects and their guards, simplifies boolean comparisons, inlines exact boolean-materialization helpers, normalizes negation into guard polarity, and deduplicates equivalent predicates inherited through nested calls. This is still structural: guards are conjunctions of necessary control predicates, not a claim that every opaque call inside them is already lowered.

On the real vanilla Hopper `updateEntity` path this now yields:

- the cooldown decrement guarded by non-null world, server-side execution, and cooldown `> 0`;
- `setCooldown(0)` guarded by non-null world, server-side execution, and concrete `cooldown <= 0` (the legacy `func_145888_j()` helper is gone);
- the successful-transfer `setCooldown(8)` path inherits the same `cooldown <= 0` predicate and lowers another static helper to its metadata bit test;
- the final transfer-success local `l1` is now reconstructed as a guarded piecewise value by walking complete entry-to-use CFG paths while carrying the last assignment. This handles initialize-then-conditionally-overwrite compiler shapes without pretending assignment-site guards alone describe the value at use time;
- binary predicates over piecewise values are expanded into boolean formulas, common path guards are factored, and the real Hopper cooldown-8 condition now contains neither an unresolved local nor a remaining piecewise node.

Furnace recovery also benefits immediately: burn-time decrement is structurally guarded by burn time `> 0`, while cook-time increment carries the server-side, input/fuel presence, burn-active, and can-smelt predicates. Piecewise path reconstruction is bounded and fail-closed: relevant CFG cycles or branching switches are not guessed.

## Clocked-state candidates

`JimpleProgram::clocked_state_candidates()` and the `jimple-clocked` CLI now combine direct temporal-counter recovery with the interprocedural guarded-effect summary. The result deliberately does **not** assert that a fixed-delta field is a timer. It records the evidence needed by a later lowering layer: delta per entry activation, activation guards, other guarded writes to the same state, direct comparisons, guard predicates that observe the state, and whether the receiver is the entry `this` object.

The three vanilla samples expose why that distinction matters:

- Hopper cooldown: one entry-owned candidate, delta `-1`, helper-mediated writes `0/8`, and two composed guards that observe the cooldown.
- Brewing time: one entry-owned candidate, delta `-1`, writes `0/0/400`, direct `<= 0`/`!= 0` comparisons, and four observing guards.
- Furnace: entry-owned burn (`-1`) and cook (`+1`) candidates **plus** `ItemStack.stackSize -= 1` on a nested receiver. The latter is a resource mutation with the same superficial fixed-delta shape, so the analyzer exposes it rather than naming every decrement a clock.

Activation cadence remains external semantic evidence. Once a caller knows that one entry activation is 50 ms, Hopper's recovered `-1`, readiness-at-`<=0`, and successful reset-to-`8` can be represented as a 400 ms absolute deadline. Without that cadence evidence the IR remains per-activation rather than inventing time.


## Scheduled callback evidence

The repeater/torch family is deliberately different from the polling-counter cases. The recovered block code already calls the legacy world scheduler, so the target should preserve that callback contract rather than inventing a deadline field on the block.

Inspection of the patched vanilla 1.7.10 `WorldServer` / `NextTickListEntry` sources gives the exact queue semantics:

- pending identity is `(x, y, z, block)`; scheduled time and priority are **not** part of equality;
- if that key is already pending, a second scheduling request is ignored rather than replacing or moving it;
- ordering is scheduled time, then lower numeric priority, then insertion ID;
- `BlockRedstoneDiode.updateTick` makes a guarded `scheduleBlockUpdateWithPriority(...)` call with priority `-1`; its delay remains a symbolic call of current metadata/state and can be lowered separately;
- the world removes an entry from the pending set before invoking the callback, so the callback can schedule the same key again;
- callback execution revalidates that the expected block still occupies the position;
- unloaded positions are rescheduled, and vanilla drains at most 1000 pending block ticks per world tick.

The Jimple effect IR now preserves **all** composed calls as first-class call sites with substituted guards/receiver/arguments and a resolution status (`expanded`, `missing_method`, `dynamic_dispatch`, or `recursive_cycle`). `jimple-calls` can filter this surface without parsing a giant effect dump. The diode scheduler appears as exactly one `dynamic_dispatch` boundary on `World`, with four guards, priority `-1`, and its delay expression intact. Refusing to guess `WorldServer` dispatch is useful here because the call is exactly where a source-API → engine-primitive binding belongs.

The diode delay is correctly polymorphic rather than merely unsimplified: base `BlockRedstoneDiode.func_149899_k(meta)` delegates to abstract `func_149901_b(meta)`. `BlockRedstoneRepeater` implements metadata-dependent `repeaterState[(meta & 12) >> 2] * 2`, while `BlockRedstoneComparator` returns constant `2`. The exporter now carries superclass/interface metadata and the Rust structural analyzer accepts an exact runtime `this` type, walks that hierarchy for virtual/interface dispatch, preserves the concrete receiver through inherited wrappers, and inlines bounded side-effect-free symbolic returns. On the real bytecode, binding the diode root to `BlockRedstoneRepeater` recovers the powered-repeater block identity plus `repeaterState[(metadata & 12) >> 2] * 2`; binding it to `BlockRedstoneComparator` recovers the powered-comparator identity plus literal delay `2`. The abstract diode analysis still refuses to choose either one.

Source APIs are connected to engine primitives through declarative binding descriptors rather than method-name rules. `JimpleScheduleOnceBinding` identifies a source method signature and the argument positions forming the deduplication key, delay, and optional priority. Applied to the vanilla world scheduler, args `0..3` become `(x, y, z, block)`, arg `4` is the specialized delay expression, and arg `5` is priority `-1`; the original guards and call-resolution provenance are retained on the bound candidate.

Quiescence has the corresponding domain-neutral primitive: keyed schedule-once reaction reevaluations survive ordinary dependency invalidation, preserve the first accepted deadline for duplicate keys, and order same-time callbacks by priority then insertion sequence. Minecraft-specific block revalidation, chunk policy, source-tick-to-time conversion, and the legacy 1000-per-world-tick budget remain adapter policy rather than generic graph semantics.

## Stochastic opportunity evidence

Crop growth confirms a third execution family distinct from polling clocks and keyed scheduled callbacks. Vanilla random ticks have two separate randomness sources that must not be conflated:

- `WorldServer.func_147456_g()` selects candidate block positions by advancing the world `updateLCG` state. For each active `ExtendedBlockStorage` needing random ticks, the compiled CFG contains a natural induction loop `i = 0; i < 3; ++i`. `JimpleMethod::fixed_trip_loops()` recovers that loop structurally and proves exactly **3 iterations** without knowing Minecraft names. Each iteration advances `updateLCG = updateLCG * 3 + 1013904223`, extracts three coordinate nibbles, tests the selected block's random-tick flag, and may dispatch `Block.updateTick(...)`.
- `BlockCrops.updateTick()` receives the world's separate `java.util.Random` instance and, only after its deterministic light/stage guards, performs `nextInt((int)(25 / growthFactor) + 1)`. A declarative `JimpleUniformIntBinding` now lifts that source API call into a uniform integer draw while preserving the RNG receiver as stream identity, the symbolic exclusive bound, guards, and call-resolution provenance. On the real crop handler this yields one draw on parameter 4 with exactly the source growth bound.

This argues against per-crop stochastic clocks. The dense world selector should remain a specialized source that owns its RNG state/order and emits causal callback opportunities. Handler-local random draws remain explicit effects on the distinct RNG stream when those callbacks execute. The graph only needs an unkeyed external-event injection surface for emitted callbacks; it does not need to own or statistically approximate the legacy random-tick selector.

## Bounded temporal-history evidence

Redstone-torch burnout supplies the fourth temporal family: bounded history rather than a polling counter, one-shot callback, or stochastic source. The compiled vanilla code now recovers enough structure to state the mechanism without torch-specific analyzer rules:

- JVM `cmp`-to-zero scaffolding is normalized back to ordinary comparisons. The real prune loop therefore exposes `world_time - oldest_timestamp > 60` directly rather than a synthetic `cmp` temporary.
- `JimpleMethod::local_counter_candidates()` recovers the helper accumulator as `count = 0`, guarded `count += 1`, with a constant threshold observation at `8`.
- After the loop-control-dependence fix, that increment carries exactly the same-iteration guards needed for a matching record: the list scan is still active and all three stored coordinate fields equal the requested `(x, y, z)`. It no longer inherits the later `count < 8` test or unrelated map/append control flow.
- The record constructor call carries exactly `(x, y, z, current_world_time)` and is guarded by the helper's boolean "record this toggle" parameter.
- The outer update lazily removes index `0` while age is greater than `60`; age exactly `60` remains in the window.

The resulting source-backed semantic object is therefore a per-owner ordered event history keyed by `(x, y, z)`: lazily retain events with age `<= 60`, optionally append the current keyed event, and report burnout once the retained key count reaches `8`. No expiry callback is required because vanilla observes/prunes the history only when another torch callback occurs.

Quiescence now mirrors that contract with graph-owned lazy event windows. A reaction can query the current keyed count and record a new event in the same transition as ordinary state writes. Window expiry is observational rather than scheduled: old records simply stop counting when queried and are physically pruned on later records. The torch-shaped runtime example reaches `toggle_count=8`, sets `burned_out=1`, has no pending expiry event, and later observes the count decay to zero solely because simulated time advanced.

## Indexed resource transaction surface

Hopper pull/extraction exposes why graph-visible field-by-field mutation would be the wrong abstraction. Vanilla speculatively removes one item from a source inventory, passes that mutable value through nested insertion logic, and restores the original source slot if insertion is incomplete. Intermediate `ItemStack.stackSize` mutations are implementation detail, not committed world state.

The structural bridge is therefore source-API driven and domain-neutral:

- `JimpleIndexedStoreBinding` maps source calls onto generic indexed-store operations (`read`, `take`, `write`, `touch`) by declaratively selecting the store operand, key operands, optional value, and optional amount. The analyzer contains no `IInventory` or `ItemStack` names.
- Applied to real `TileEntityHopper.func_145892_a`, the vanilla descriptor yields **12** operations: 3 reads, 1 take, 3 writes, and 5 touches. Symbolic participants separate cleanly into source inventory parameter 1 and destination hopper/store parameter 0.
- `JimpleSnapshotCloneBinding` describes source value-clone APIs. A conservative rollback detector requires the take and restore to target the exact same symbolic store/key in the same method scope, the restore to occur later and include every take guard, and the restore value to be a configured clone of a configured pre-take read from that exact location.
- On the real helper this produces exactly **one** speculative rollback: source parameter 1, slot parameter 2, amount literal `1`, take statement 9, restore statement 16, with the restore value mechanically proven as `copy(read(source, slot))`. The restore path has a strict guard superset (2 guards on take, 4 on restore).
- Synthetic regression coverage rejects a restore to the wrong key and a restore missing the take guard.

`indexed_store_transaction_coverage()` now closes that semantic surface without teaching the analyzer Minecraft types. A single `JimpleTransactionBindings` descriptor groups indexed-store operations, snapshot clones, value components, pure value operations, store queries, and object-state writes. For the real pull helper the current report is `complete=true` with **12** indexed-store operations, **1** proven rollback, **4** transaction-local component updates, **16** pure value operations, **7** store policy/topology queries, and **2** object-state writes; there are **0** unaccounted field writes and **0** unaccounted calls.

The four raw `ItemStack.stackSize` writes are bound as updates to an immutable `quantity` component only when the mutated receiver is structurally proven to originate from a configured store `read` or `take`. They therefore become private value transforms inside the transaction rather than graph-visible heap writes. The 16 pure value operations are 12 component queries (`item_identity`, `metadata`, `max_quantity`), two equality tests, and two `min` operations. The seven remaining inventory calls lower to four `allows_write` predicates, one `allows_read` predicate, one side-qualified `keys` query, and one `extent` query. The two remaining Hopper calls lower to generic `cooldown = 8` object-state writes with their original branch guards.

`complete=true` is deliberately a **surface-completeness** claim, not yet an execution claim: every externally visible write and unresolved source-boundary call reached by the structural summary has a typed semantic mapping. The transaction still has dynamic control flow that must be preserved rather than flattened into an unordered bag of possible effects.

That control flow is now recovered as well. `JimpleMethod::induction_loops()` generalizes the earlier fixed-trip loop analysis to constant initialization/step with a symbolic bound; `fixed_trip_loops()` is the strict constant-bound subset. It also handles compiler layouts where a loop backedge lands on a straight-line bound-recompute block before the controlling `if`. On the real insertion helper this recovers both scans: the sided path is `i = 0; ++i` over `accessible_slots(side).length`, while the ordinary path is `i = 0; ++i` over `inventory.extent`. Interprocedural composition substitutes the root Hopper arguments and guards, and the transaction binder normalizes those bounds to `store_keys_length(store, side)` and `store_extent(store)`.

The intended native lowering remains coarse: execute the bound branch/loop/value/store semantics against immutable or transactional canonical snapshots, then publish only committed resource snapshots plus success/cooldown at the graph boundary. The graph should never observe the speculative mutable `ItemStack` implementation sequence. The next frontier is therefore an executable control-flow-preserving transaction IR/codegen/native-store adapter, not more source-API archaeology.

## Missing pieces exposed by vanilla

The current graph runtime proves the scheduling model but intentionally still has prototype numeric state. Exact Minecraft lowering needs:

- a control-flow-preserving executable transaction IR/codegen path that consumes the now-complete store/value/query/state bindings and runs them against native immutable snapshots;
- adapter lowering from recovered source calls/effects into concrete world transactions, including block revalidation, chunk policy, and source-time conversion;
- native store implementations for indexed resource reads/writes, policy queries, and atomic commit publication into graph-owned canonical handles;
- source/runtime-type evidence for polymorphic world objects and modded subclasses wherever static bytecode alone cannot prove dispatch;
- preservation of source RNG state/order in dense adapters while bound random draws and emitted callbacks cross the semantic IR boundary.

The important result of this spike is that none of these require reintroducing a global tick loop. They are additions to the causal graph interface.
