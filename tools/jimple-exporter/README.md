# Jimple exporter spike

Experimental JVM-bytecode frontend for `minecraft-redox`.

The exporter deliberately does **not** contain GregTech semantic rules. SootUp lowers already-compiled Java bytecode to Jimple and this tool emits a small JSON boundary containing statement kinds, definitions, typed uses, and CFG successors. The intended consumer is a Rust abstract interpreter in `minecraft-redox`.

This lets the Rust side reason about finite Java execution rather than recognizing source-level vocabulary such as `getDust`, recipe metadata key names, or individual machine classes.

## Build

```sh
mvn -q -DskipTests compile
```

## Structured export

```sh
mvn -q exec:java \
  -Dexec.mainClass=dev.minecraftredox.jimple.DumpJson \
  -Dexec.args="/path/to/classes fully.qualified.ClassName optionalMethodSubstring"
```

For interprocedural experiments, `DumpReachable` follows concrete bytecode callees and class initializers from one root method while stopping at runtime-library boundaries already modeled by the Rust VM:

```sh
mvn -q exec:java \
  -Dexec.mainClass=dev.minecraftredox.jimple.DumpReachable \
  -Dexec.args="/path/to/classes fully.qualified.RootClass methodNameSubstring 2000"
```

The final argument is a hard method-count guard. This is intentionally a bounded analysis export, not an invitation to serialize the entire JVM.

Each concrete method is emitted as JSON with its signature and statements. Every statement currently includes:

- stable per-method statement ID,
- SootUp statement kind,
- textual form for diagnostics,
- optional definition,
- typed uses,
- CFG successor statement IDs,
- structured statement operations, and
- recursively structured values for locals, constants, fields, arrays, arithmetic, casts, allocations and calls.

`DumpJimple` is retained as a human-readable debugging companion.

## GTNH spike results

The prototype has been exercised against compiled `GT5-Unofficial` bytecode and exposes, without source-vocabulary adapters:

- the complete `GTOreDictUnificator.getDust(Materials, long)` threshold/control-flow logic,
- `ItemStack.copy()` as allocation plus item/count/damage/NBT value copying,
- the bounded `MaskList.values()` loop that constructs and fills `LanthItemList.maskMap`,
- the bounded loop that constructs every `Loaders.NeutronAccelerators[i]`, and
- the Transcendent Plasma Mixer dataflow from opaque recipe metadata plus recipe fields to stored energy, followed by the wireless balance mutation and explicit zeroing of calculated EU/t.

The Rust side now consumes this structure directly. It has four generic analysis modes:

- backward provenance slicing plus symbolic expression reconstruction,
- bounded finite execution over concrete integer control flow, allocations, object constructors, arrays, field writes and opaque symbolic calls,
- interprocedural execution over multiple Jimple classes in one shared abstract JVM state, including modeled JDK map operations, runtime-class interface dispatch, reference-preserving casts, throws, and branch-state joins,
- JVM-style one-shot class initialization on reached static use/allocation, with recursion protection for mutually-referential `<clinit>` code, and
- exact helper-call execution with caller-supplied abstract values, including after an initialization/registration sequence in the same heap, so branchy utility methods can be evaluated against recovered startup state instead of an empty registry.

Unknown conditionals are joined at their immediate normal post-dominator when one exists. This is important for bytecode such as optional diagnostics: the two diagnostic paths are evaluated separately, their state is joined, and the shared semantic tail executes only once. Allocations after a diamond therefore retain one identity instead of becoming a false `A | B` uncertainty.

Calls whose implementation is not present remain explicit `opaque_call_shapes` containing the method signature, receiver provenance and argument values. They are evidence about reached call edges, not a claim that the opaque callee was executed.

Real-corpus acceptance tests currently include:

- `Loaders.GTMetaTileRegister()` materializes exactly 13 writes to `Loaders.NeutronAccelerators[0..12]` without GoodGenerator-specific rules. Each entry retains its distinct `MTENeutronAccelerator` allocation, constructor tier argument `0..12`, and `getStackForm(1)` result.
- Calling only `LanthItemList.registerTypical()` now triggers the required class initializers mechanically, executes 3,724 statements, and materializes all 34 `maskMap` entries without Lanthanides-specific rules or an explicit startup sequence. Every key retains identity with its `MaskList` static enum field; e.g. `BLANK1` maps to a distinct `ItemPhotolithographicMask("blank1", "VISIBLE")` allocation.
- A real `bartworks.common.loaders.recipes.FluidHeater.run()` recipe executes through `GTValues.RA -> RecipeAdder.stdBuilder() -> GTRecipeBuilder` in 1,598 abstract statements. The resulting builder has concrete `duration = 90`, `eut = 120`, one concrete fluid-input array and one concrete fluid-output array. Their `FluidStack` elements preserve constructor provenance `FluidLoader.fulvicAcid, 1000` and `FluidLoader.heatedfulvicAcid, 1000`. The unresolved registration edge is retained as `RecipeMaps.fluidHeaterRecipes -> IRecipeMap.doAdd(builder)`.
- Exact execution of the real `GTOreDictUnificator.getDust(Materials, long)` with `Electrum` and `2*M` mechanically derives the preferred `get(dust, Electrum, 2)` lookup and its source fallbacks `dustSmall x8` and `dustTiny x18`. The final result deliberately remains conditional while the ore registry is absent; this demonstrates why registry-state recovery is preferable to assuming the preferred representation always exists.

The next frontier is startup/registry-state execution. GregTech's unificator keeps a canonical `sName2StackMap` populated by registration code before falling back to Forge's ore dictionary, so a sufficiently complete initialization heap should make many helper results concrete without teaching the Rust interpreter helper names.
