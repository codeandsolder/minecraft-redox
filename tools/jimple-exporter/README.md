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
- CFG successor statement IDs.

`DumpJimple` is retained as a human-readable debugging companion.

## GTNH spike results

The prototype has been exercised against compiled `GT5-Unofficial` bytecode and exposes, without source-vocabulary adapters:

- the complete `GTOreDictUnificator.getDust(Materials, long)` threshold/control-flow logic,
- `ItemStack.copy()` as allocation plus item/count/damage/NBT value copying,
- the bounded `MaskList.values()` loop that constructs and fills `LanthItemList.maskMap`,
- the bounded loop that constructs every `Loaders.NeutronAccelerators[i]`, and
- the Transcendent Plasma Mixer dataflow from opaque recipe metadata plus recipe fields to stored energy, followed by the wireless balance mutation and explicit zeroing of calculated EU/t.

The next step is intentionally not more Java: consume this JSON from Rust and implement finite abstract execution over values, heap objects, arrays/maps, branches, loops, and calls.
