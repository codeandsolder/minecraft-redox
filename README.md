# minecraft-redox

> Nice Java you've got there. Would be a shame if somebody learned what it wants to do.

`minecraft-redox` is a semantic excavation compiler for Minecraft and modded-Minecraft codebases. It is **not** a Java-to-Rust transpiler. Its job is to recover intent from historical implementations and account for that intent in clean, reusable behavior/content representations.

The target architecture is deliberately split from both Minecraft and GregTech:

```text
Minecraft / Forge / mods / datapacks / bytecode
                    ↓
          parser frontends
                    ↓
      semantic classification
                    ↓
       normalized semantic IR
                    ↓
  behavior/content lowering passes
                    ↓
 ThingsABlockGameNeeds + game content
```

Every source unit should eventually have a provenance-backed disposition:

- **Behavior** — executable semantics expressed through target behavior contracts.
- **Content** — recipes, materials, structures, loot, registries, etc.
- **EnginePrimitive** — repeated semantics promoted into reusable engine machinery.
- **Drop** — compatibility/runtime surgery that has no counterpart in the clean engine.
- **Oracle** — retained characterization/differential test behavior.
- **Unresolved** — we do not yet know what it means. This is explicit, never silently ignored.

## Language policy

The implementation is Rust. Java, Kotlin, Scala, bytecode, JSON, datapacks, etc. are **inputs**, each behind a frontend. No JVM language is required as a foundation dependency.

The first frontend inventories Java with tree-sitter and preserves precise source provenance. Subsequent passes will recover higher-level idioms (registries, recipe builders, machine state, events, Mixins, ASM/coremods, persistence, rendering scripts, etc.) into the semantic IR.

## Relationship to the other projects

- **Quiescence**: general temporal/incremental computation engine.
- **ThingsABlockGameNeeds**: shared block-game substrate and behavior contracts.
- **minecraft-redox**: archaeology and semantic recovery from Minecraft-shaped historical implementations.
- **GregTech: Oxidized Horizons**: one game/content profile consuming all of the above.

The earlier `gtnh-redox` experiments are prior art and a source of useful heuristics, not the architectural boundary. GTNH-specific recognition belongs as lowering/classification modules on top of this core.

## Current executable milestone

The first GTNH lowering pass is implemented in `minecraft-redox-gtnh`: receiver-chain recovery plus import/static-import name resolution now turns GT5 `GTRecipeBuilder` and `MaterialBuilder` chains into provenance-backed content records. The measured GT5 source tree currently yields 5,791 recipe builders and 885 material builders with every builder call structurally accounted for. Nested Java expressions remain explicitly source-shaped until the expression/type lowering layer resolves them; structural builder coverage is not claimed as full gameplay parity.

See [`docs/gt5-first-pass.md`](docs/gt5-first-pass.md) for the methodology and current boundary.
