# Architecture

## Non-goal: source translation

Source syntax is evidence. The desired output is not structurally similar code; it is an accounting of observable semantics.

A Mixin that changes cloud height may lower to a behavior parameter. A giant recipe-builder class may lower to content rows. An ASM transformer may lower to `Drop` because the clean engine exposes the intended behavior directly. A genuine algorithm may lower to executable behavior plus differential tests.

## Pipeline

1. **Acquire** versioned source/data/bytecode with hashes and artifact provenance.
2. **Parse** each representation into syntax/symbol inventories without semantic guessing.
3. **Resolve** symbols/types/call sites where useful.
4. **Recognize** known implementation idioms into candidate semantic units.
5. **Normalize** candidates into stable behavior/content concepts.
6. **Validate** with static invariants, compilation, fixtures, and old-runtime differential oracles.
7. **Account** every source unit in the provenance ledger. Unknown semantics stay explicitly unresolved.

## Frontend boundary

Frontends emit source facts and provenance. They do not know GregTech or the target engine. Java is first because it dominates the historical corpus. Kotlin and Scala can be added as sibling frontends without changing the semantic IR.

## Lowering boundary

Domain-specific recognizers are layered modules. Examples:

- vanilla registries and behavior implementations
- Forge event/lifecycle idioms
- Mixin/ASM intent recovery
- GT recipe/material builders
- GregTech machine/process patterns
- GUI/render compatibility machinery

Repeated exceptions should improve the target abstraction or deterministic recognizer before they become one-off generated code.

## Required metrics

Keep these separate:

- parsed
- classified
- semantically resolved
- generated/lowered
- compiled
- oracle validated

A high parser/classifier percentage is not parity.
