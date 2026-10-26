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

The Rust side now consumes this structure directly. It has three generic analysis modes:

- backward provenance slicing plus symbolic expression reconstruction,
- bounded finite execution over concrete integer control flow, allocations, object constructors, arrays, field writes and opaque symbolic calls, and
- interprocedural execution over multiple Jimple classes in one shared abstract JVM state, including modeled JDK map operations and reference-preserving casts.

Real-corpus acceptance tests currently include:

- `Loaders.GTMetaTileRegister()` materializes exactly 13 writes to `Loaders.NeutronAccelerators[0..12]` without GoodGenerator-specific rules. Each entry retains its distinct `MTENeutronAccelerator` allocation, constructor tier argument `0..12`, and `getStackForm(1)` result.
- `MaskList.<clinit>()` + `LanthItemList.<clinit>()` + `LanthItemList.registerTypical()` executes 3,316 statements and materializes all 34 `maskMap` entries without Lanthanides-specific rules. Every key retains identity with its `MaskList` static enum field; e.g. `BLANK1` maps to a distinct `ItemPhotolithographicMask("blank1", "VISIBLE")` allocation.

The next frontier is executing recipe-builder implementations themselves, so recipe registration can be recovered from object state and call effects rather than source-level builder vocabulary.
