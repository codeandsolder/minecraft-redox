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

The Rust side now consumes this structure directly. It has two generic analysis modes:

- backward provenance slicing plus symbolic expression reconstruction, and
- bounded finite execution over concrete integer control flow, allocations, object constructors, arrays, field writes and opaque symbolic calls.

As a real-corpus acceptance test, the finite executor runs `Loaders.GTMetaTileRegister()` without GoodGenerator-specific rules and materializes exactly 13 writes to `Loaders.NeutronAccelerators[0..12]`. Each entry retains its distinct `MTENeutronAccelerator` allocation, constructor tier argument `0..12`, and `getStackForm(1)` result.

The next frontier is interprocedural execution and generic models for common JDK collection behavior so registry maps such as `LanthItemList.maskMap` can be materialized in the same way.
