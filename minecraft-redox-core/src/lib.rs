//! Shared semantic IR and provenance types for Minecraft Redox.
pub mod content;
pub mod transaction;

use serde::{Deserialize, Serialize};

/// Source language or representation from which a semantic unit was recovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SourceKind {
    Java,
    Kotlin,
    Scala,
    Bytecode,
    Data,
}

/// A precise reference back to archaeological source material.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceOrigin {
    pub source_kind: SourceKind,
    pub artifact: String,
    pub version: Option<String>,
    pub path: String,
    pub symbol: Option<String>,
    pub byte_start: Option<usize>,
    pub byte_end: Option<usize>,
}

/// What happened to a recovered semantic unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Disposition {
    /// Executable game behavior expressed through a target behavior contract.
    Behavior,
    /// Declarative content such as recipes, materials, structures, or loot.
    Content,
    /// Behavior that belongs in a reusable engine primitive.
    EnginePrimitive,
    /// Historical compatibility machinery intentionally absent from the target.
    Drop,
    /// Source retained as an executable/differential characterization oracle.
    Oracle,
    /// Not yet understood well enough to classify.
    Unresolved,
}

/// Confidence represented without floating-point state: 0..=1000 permille.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ConfidencePermille(u16);

impl ConfidencePermille {
    pub const MAX: u16 = 1000;

    /// Returns `None` for values outside 0..=1000.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// One piece of evidence supporting a semantic classification or lowering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub origin: SourceOrigin,
    pub reason: String,
    pub confidence: ConfidencePermille,
}

/// Stable semantic accounting record. Every excavated source unit eventually gets one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SemanticUnit {
    pub id: String,
    pub disposition: Disposition,
    pub target: Option<String>,
    pub dependencies: Vec<String>,
    pub evidence: Vec<Evidence>,
}
