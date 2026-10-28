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
- if a dequeued callback position is no longer available, vanilla attempts a zero-delay requeue through the normal scheduler (which applies its chunk-existence gate again); vanilla drains at most 1000 pending block ticks per world tick.

The Jimple effect IR now preserves **all** composed calls as first-class call sites with substituted guards/receiver/arguments and a resolution status (`expanded`, `missing_method`, `dynamic_dispatch`, or `recursive_cycle`). `jimple-calls` can filter this surface without parsing a giant effect dump. The diode scheduler appears as exactly one `dynamic_dispatch` boundary on `World`, with four guards, priority `-1`, and its delay expression intact. Refusing to guess `WorldServer` dispatch is useful here because the call is exactly where a source-API → engine-primitive binding belongs.

The diode delay is correctly polymorphic rather than merely unsimplified: base `BlockRedstoneDiode.func_149899_k(meta)` delegates to abstract `func_149901_b(meta)`. `BlockRedstoneRepeater` implements metadata-dependent `repeaterState[(meta & 12) >> 2] * 2`, while `BlockRedstoneComparator` returns constant `2`. The exporter now carries superclass/interface metadata and the Rust structural analyzer accepts an exact runtime `this` type, walks that hierarchy for virtual/interface dispatch, preserves the concrete receiver through inherited wrappers, and inlines bounded side-effect-free symbolic returns. On the real bytecode, binding the diode root to `BlockRedstoneRepeater` recovers the powered-repeater block identity plus `repeaterState[(metadata & 12) >> 2] * 2`; binding it to `BlockRedstoneComparator` recovers the powered-comparator identity plus literal delay `2`. The abstract diode analysis still refuses to choose either one.

Source APIs are connected to engine primitives through declarative binding descriptors rather than method-name rules. `JimpleScheduleOnceBinding` identifies a source method signature and the argument positions forming the deduplication key, delay, and optional priority. Applied to the vanilla world scheduler, args `0..3` become `(x, y, z, block)`, arg `4` is the specialized delay expression, and arg `5` is priority `-1`; the original guards and call-resolution provenance are retained on the bound candidate.

Quiescence has the corresponding domain-neutral primitive: keyed schedule-once reaction reevaluations survive ordinary dependency invalidation, preserve the first accepted deadline for duplicate keys, and order same-time callbacks by priority then insertion sequence. Source-tick-to-causal-time conversion is explicit in the Redox adapter; Minecraft-specific block revalidation, chunk policy, immediate-update behavior, and the legacy 1000-per-world-tick budget remain adapter policy rather than generic graph semantics.

The source-neutral transaction CFG now carries that scheduling boundary as `TransactionEffect::ScheduleOnce { key, delay, priority }`. The Jimple transaction lowerer maps the bound world-scheduler call directly to this effect; the isolated transaction draft only publishes schedule requests on successful commit, so failed/speculative execution cannot leak callbacks. Persistent schedule keys are canonical structural values: scalar/sequence values intern directly, while transaction handles are rejected unless the adapter is explicitly configured with an immutable identity component such as `semantic_id`. The Redox→Quiescence adapter then converts source delay units through an explicit `SimDuration` unit and interns the composite key to Quiescence's `u64` keyed-callback identity. `quiescence@339c89a` adds an explicit targeted keyed-callback request, so a source producer can schedule a distinct callback reaction instead of incorrectly re-running itself; this is required for vanilla neighbor-change code scheduling a later `updateTick` handler.

The real repeater neighbor-change helper (`BlockRedstoneDiode.func_149897_b`) now lowers completely for exact runtime type `BlockRedstoneRepeater`: **10 anonymous transaction functions / 161 CFG statements**, with no `net.minecraft`, SRG method/field name, or Jimple invoke-kind leakage in the executable target. Its semantic coverage report is also complete: 24 bound host-query call sites, one bound schedule-once effect, three bound pure-value operations, and **zero** unaccounted calls or field writes. This path forced several generic compiler fixes rather than vanilla-specific exceptions: inherited static method resolution through the declared-owner hierarchy, literal lowering of explicitly bound stable static identities, integer `switch` CFG nodes, `Math.max`, and integer shift operators used by metadata/direction math.

Differential execution of that recovered repeater transaction exercises all three vanilla priority branches with the same key `(10, 64, 20, repeater)` and metadata `4`, whose delay is mechanically recovered as `repeaterState[1] * 2 = 4` legacy ticks:

- ordinary input transition: **140 statements**, delay `4`, priority **-1**;
- powered repeater losing input: **143 statements**, delay `4`, priority **-2**;
- locked repeater: **154 statements**, delay `4`, priority **-3**.

The Quiescence bridge converts those four source ticks with the configured 50 ms unit to **200 ms**. A queue-level regression uses separate producer and callback reactions: the recovered neighbor handler emits the same transaction-generated key once at time 0 and again at 50 ms, both targeting the callback reaction. The duplicate does not replace/postpone the first pending request, the producer is not re-run by the scheduled event, and the callback still fires at 200 ms. This mechanically matches the vanilla `HashSet` pending-entry behavior rather than merely reproducing its nominal delay.

Comparator specialization independently confirms that the same scheduler binding is polymorphic rather than repeater-specific. For exact runtime type `BlockRedstoneComparator`, the real neighbor-change helper yields two guarded schedule candidates with the same `(x, y, z, this)` key, literal delay **2** source ticks (**100 ms** at the vanilla tick unit), and priorities **-1** or **0** according to the comparator's source branch.

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

The control-flow lowering is now executable-shaped and source-neutral. The Jimple exporter preserves `instanceof` structurally; declarative capability bindings map source runtime types onto target capability IDs such as `sided_access` and `transfer_cooldown`. `minecraft-redox-core::transaction` defines a compact CFG of anonymous numeric function/statement/local IDs, generic arithmetic/control flow, semantic store/value/state operations, and capability tests. Exact in-program static/special helpers compile into anonymous transaction functions instead of being flattened or retaining source method identities.

Applied to the real Hopper pull path, `jimple-lower-transaction` produces **6 anonymous functions / 132 CFG statements** while retaining the source branches, returns, speculative take/restore path, and both dynamic insertion scans. The serialized transaction program contains **zero** `net.minecraft`, SRG `func_*`/`field_*`, JVM invoke-kind, `ItemStack`, `IInventory`, `ISidedInventory`, or `TileEntityHopper` names. A synthetic regression separately requires an `instanceof` source type to become a semantic capability check and requires exact helper names/types to disappear from serialization.

The native execution boundary now exists as well. `TransactionHost::begin()` creates an isolated `TransactionDraft`; the generic executor interprets anonymous functions/locals/branches/calls against that draft and invokes `commit(self)` only after a successful top-level return. Dropping the draft after any interpreter/host error leaves externally visible state unchanged. Domain values are opaque handles, so source-style aliases can observe transaction-local component mutation without publishing those objects. Unit tests cover successful commit versus execution-error discard and an explicit snapshot → speculative take → private mutation → source restore sequence.

A serializable source-neutral `MemoryTransactionHost` provides a differential/reference implementation with component-map objects, indexed stores, capabilities, semantic equality tags, state writes, and configurable quantity semantics. The generic `transaction-run-memory` CLI has no Jimple/source dependency once it receives a `TransactionProgram`. Running the real 6-function/132-statement Hopper program against this host now gives two end-to-end acceptance cases: with five source items and an empty destination it returns `1` after **76** CFG statements, leaves source quantity `4`, publishes destination quantity `1`, touches both stores, and writes cooldown `8`; with every destination slot full/incompatible it returns `0` after **229** CFG statements, leaves the destination unchanged, performs no cooldown/touch effects, and restores the source to quantity `5` using the independent pre-take snapshot rather than the speculatively decremented object.

The graph publication boundary is now implemented in the separate `minecraft-redox-quiescence` adapter crate, pinned to `quiescence@339c89a` so Quiescence itself remains domain-agnostic. Graph atom IDs name immutable canonical store snapshots. The adapter materializes only transaction-local handles inside a private draft, runs the source-neutral program, re-canonicalizes committed stores, interns them, and returns ordinary `StateWrite::atom` updates. A surrounding reaction can still check its deadline before calling the adapter, so store atoms are not read—and therefore do not become dependencies—during cooldown.

The canonical snapshot interner uses a structural ordered key rather than a linear historical scan. Canonical components reject ephemeral transaction handles at the boundary; floating-point components, if present, use exact IEEE bit identity. Re-interning an unchanged take→restore store therefore returns the original atom ID and emits no graph write. Regression tests prove both that unchanged canonical stores are elided and that store dependencies appear only after the deadline-gated transaction actually runs.

The real 6-function/132-statement Hopper pull program now crosses the complete path into Quiescence. Starting with source quantity `5`, an empty five-slot destination, and `ready_at=400 ms`, the recovered reaction reaches the causal boundary, executes the source-derived transaction, interns source quantity `4` and destination quantity `1` snapshots, and publishes those two atom handles plus exact next deadline `800 ms` in one reaction transition. No transaction-local object handle escapes into canonical state.

The next hard frontier is therefore **resource discovery/topology**, not transaction execution or publication. The current acceptance feeds the pull helper its source/destination stores explicitly. Vanilla's outer Hopper logic discovers neighboring inventories/world objects and chooses push/pull paths. Those world/topology queries should become typed semantic bindings that resolve to graph-owned topology/resource handles while preserving the same dynamic-dependency discipline.

## Source-neutral world/topology discovery

The transaction boundary now covers resource discovery as well as resource mutation. `TransactionValue::HostQuery` carries only a semantic operation ID, optional receiver, and source-neutral operands; `TransactionDraft::host_query` is allowed to be stateful so topology reads and RNG draws can share one mechanism without pretending randomness is pure. `TransactionValue::Sequence` plus ordinary length/index operations represent bounded regions and query result collections without carrying `AxisAlignedBB` or `java.util.List` into the target.

Jimple bindings are declarative and selective. A host-query binding names the source signature and exactly which receiver/arguments survive semantically. Unselected source operands are not lowered. A binding-aware liveness pass then removes pure local scaffolding that exists only to prepare discarded source arguments; this is what erases `IEntitySelector.selectInventories` from the real Hopper discovery path rather than inventing a target selector constant for it.

Primitive JVM casts are now separate from reference-capability casts. The source-neutral IR has explicit `i32`/`f64` numeric casts, and the executor implements Java's `double -> int` narrowing edge cases plus Jimple `cmpg`/`cmpl`. CFG equality also preserves the JVM boolean/integer convention (`false == 0`, `true == 1`) while semantic item/value equality remains a separate host operation.

Applied to real vanilla `TileEntityHopper.func_145893_b`, one descriptor lowers the helper to **2 anonymous functions / 45 transaction statements** with zero source-name leakage. Its five host operations are `topology.object_at`, `topology.block_at`, `topology.composite_store_at`, `topology.inventory_objects_in_region`, and `random.uniform_int`; AABB construction lowers to a six-element sequence and list `size/get` lower to length/index. Semantic-bound source implementations are treated as transitive coverage boundaries, so AABB constructor field writes and `Object.<init>` do not reappear as false blockers. The resulting coverage is `complete=true` with zero unaccounted calls or writes.

The generic memory host executes all three recovered discovery branches:

- direct inventory tile hit -> returned store handle `2` in 38 transaction statements;
- chest-like tile + block normalization -> returned composite store handle `4` in 43 statements;
- no inventory tile -> region query returns two inventory candidates, the source RNG stream selects index 1, and handle `4` is returned in 47 statements.

Topology queries are repeatable by default in the reference draft; the configured RNG query is consumable and disappears after one draw.

The outer pull orchestration is now lowered too. `TileEntityHopper.func_145891_a(IHopper)` becomes **13 anonymous functions / 284 transaction statements**. Its complete semantic surface contains 34 indexed-store operations, 2 proven speculative rollbacks, 12 bound value-component writes (10 with direct indexed-store provenance), 64 pure value operations, 24 store queries, 7 state writes, 7 host queries, and 10 recovered loops, with zero unaccounted calls or field writes. The extra two component writes come from the loose-item-entity payload path; component-write recovery now keeps indexed-store provenance when available without requiring every private value to originate in a store.

Real memory-host execution covers the major branches. With a five-item source inventory above an empty Hopper, discovery feeds the source store directly into the recovered transfer path: return `1`, 139 executed statements, source quantity `5 -> 4`, destination `0 -> 1`, cooldown `8`, and both stores touched. With all five destination slots full of an incompatible item, all 284 statements execute, return `0`, destination/cooldown/touches remain unchanged, and the source is restored to an independent pre-take snapshot. With no inventory above, the recovered item-entity search/pickup path moves the loose stack into the Hopper, writes cooldown `8`, and consumes the stateful `entity.destroy` host operation.

`TransactionReactionAdapter` now accepts an optional reaction-time host-preparation hook. It can add graph-backed stores, transaction-local objects/capabilities, and host-operation results for that evaluation. The adapter still owns snapshot materialization, isolated execution, canonical interning, and atomic graph publication. Prepared store bindings participate in normal dynamic dependency tracking and are published exactly like fixed stores.

The `vanilla_hopper_discovered` graph example loads the real 284-statement outer-pull program. At the first 400 ms boundary it discovers the source and atomically publishes Hopper quantity `0 -> 1`, source `5 -> 4`, and deadline `800`. Replacing the source snapshot with quantity `7` at 500 ms does **not** wake the Hopper because the successful same-time reevaluation has already shrunk dependencies back to the deadline. At 800 ms the recovered transaction reads the latest source and publishes Hopper `1 -> 2`, source `7 -> 6`, and deadline `1200`. The run takes five reaction evaluations total.

The complementary push path and the complete `func_145887_i` transfer attempt are now lowered too. Two generic JVM gaps had to be closed rather than papered over with Hopper rules. First, the reachable exporter resolves concrete inherited method bodies through the superclass chain, so calls emitted against a subclass-owned signature retain implementations such as `TileEntity.getBlockMetadata()`. Second, transaction lowering accepts an optional exact runtime `this`, recovers Soot's `this := @this` local alias structurally, and performs the same hierarchy dispatch as the structural analyzer while leaving other dynamic receivers fail-closed.

Facing is recovered from startup bytecode rather than copied into the descriptor. Static primitive-array reads trigger the reachable owner `<clinit>` through the existing abstract JVM and lower materialized arrays to literal source-neutral sequences, including JVM default-filled entries. The real `Facing` opposite/X/Y/Z tables therefore disappear into constants derived from class initialization. Integer bitwise operators (`&`, `|`, `^`) execute the real metadata helpers, while generic draft-aware `empty`/`full` store queries replace Hopper's local scans at the semantic boundary.

With an exact `TileEntityHopper` runtime type and the full transaction descriptor, `func_145887_i()` lowers to **22 anonymous functions / 428 CFG statements** with zero `net.minecraft`, `java.util`, SRG `func_*`/`field_*`, `Facing`, `TileEntityHopper`, or `World` names in serialized target IR. Its current coverage is `complete=true`: **50** indexed-store operations, **2** proven rollbacks, **22** component updates, **87** pure value operations, **35** store queries, **10** state writes, **18** host-query occurrences, and **17** recovered loops, with zero unaccounted calls or field writes.

Differential execution now exercises the full vanilla attempt ordering rather than a helper in isolation:

- **push succeeds:** return `1` after 276 target statements; Hopper stack `5 -> 4`, discovered destination `0 -> 1`, cooldown `8`, Hopper and destination touched;
- **push blocked, pull succeeds:** return `1` after 322 statements; full destination unchanged, Hopper stack `5 -> 6`, discovered source `5 -> 4`, cooldown `8`, Hopper and source touched;
- **both blocked/full:** return `0` after 146 statements with all stores/state untouched.

The `vanilla_hopper_full` graph example executes that same 428-statement transaction behind the deadline gate. At 400 ms it successfully pushes one item and pulls one item in one activation, leaving Hopper quantity `5`, destination `1`, source `4`, and deadline `800`. At 500 ms both neighboring snapshots are replaced; neither wakes the cooling Hopper. At 800 ms the replacement destination is full, so push is blocked, while the latest source is pulled (`Hopper 5 -> 6`, source `7 -> 6`) and deadline advances to `1200`. The extended dependency-phase run below adds a third ready-state branch test and finishes in eight reaction evaluations.

## Missing pieces exposed by vanilla

The current graph runtime proves the scheduling model and now covers the complete vanilla Hopper transfer attempt. The next exactness gaps are at the world/adapter boundary rather than in Hopper control flow:

- Minecraft callback-adapter policy for current-block revalidation, chunk availability/requeue behavior, immediate-update mode, and the legacy 1000-callback-per-world-tick budget; source-tick-to-causal-time conversion is now implemented for bound schedule effects;
- source-neutral world-state mutation for callback bodies such as diode `updateTick` block replacement, which is intentionally separate from the already-lowered schedule producer;
- production persistent/native store implementations behind the source-neutral transaction host interface; the current memory host is a reference/differential implementation;
- source/runtime-type evidence for polymorphic world objects and modded subclasses wherever static bytecode alone cannot prove dispatch;
- preservation of source RNG state/order in dense adapters while bound random draws and emitted callbacks cross the semantic IR boundary.

The important result of this spike is that none of these require reintroducing a global tick loop. They are additions to the causal graph/adapter interface.


### Demand-driven graph-store dependencies

All graph-backed transaction stores are now demand-materialized inside the adapter rather than read eagerly at the start of a ready-phase transaction. Merely passing a store handle as the transaction receiver/argument does not inspect its atom. Execution runs against an isolated draft; if a store operation, store query, or capability test first touches an unresolved handle, the draft returns a typed `NeedStore(handle)`, the adapter reads exactly that atom (thereby adding the dependency), discards the partial draft, materializes the store, and restarts the transaction. Source-only host-query consumption and speculative mutations are replay-safe because the abandoned draft never commits. Regressions prove both halves: a fixed store argument that is never semantically accessed does not become a dependency, and among two topology-prepared stores only the store actually touched by the taken branch can wake the reaction later.
The real full-Hopper graph example exercises that branch precision too. After the second successful attempt, the Hopper snapshot is replaced during cooldown by a completely full five-slot inventory; this does not wake the reaction. At the 1200 ms deadline the full destination blocks push and the full Hopper skips pull, so only the destination remains a resource dependency. Replacing the source at 1250 ms does not wake the ready reaction, while replacing the destination at 1260 ms does. That wake executes the same recovered program: one item is pushed, the newly opened Hopper space then permits a pull from the latest source, and the next deadline becomes 1660 ms.
