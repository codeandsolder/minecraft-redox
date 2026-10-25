//! Target-neutral content IR produced by semantic lowering.

use serde::{Deserialize, Serialize};

use crate::SourceOrigin;

/// Stable logical identifier used by generated content.
///
/// The first lowering stages may preserve archaeological symbol names here.
/// Later migration passes can remap them without carrying source-language AST.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ContentId(pub String);

impl ContentId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A target-level way to identify an item independent of source syntax.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSelector {
    Named {
        id: ContentId,
        variant: Option<i64>,
    },
    MaterialForm {
        material: ContentId,
        form: ContentId,
    },
    OreKey {
        key: ContentId,
    },
}

/// A target-level fluid identity independent of Forge registry objects.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FluidSelector {
    Named {
        id: ContentId,
    },
    MaterialForm {
        material: ContentId,
        form: ContentId,
    },
}

/// One concrete fluid stack in normalized content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FluidStackSpec {
    pub selector: FluidSelector,
    pub amount: u64,
}

/// One concrete item stack in normalized content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemStackSpec {
    pub selector: ItemSelector,
    pub amount: u64,
}

/// One concrete item requirement in a recipe input slot.
///
/// `amount` is how many matching items must be present. `consumed` is separate
/// because some recipe systems use catalysts/configuration items that are
/// required for matching but survive a successful recipe execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemInputStackSpec {
    pub selector: ItemSelector,
    pub amount: u64,
    pub consumed: bool,
}

/// One recipe input slot. Alternatives occupy one slot, not several inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemInputSpec {
    Stack(ItemInputStackSpec),
    Alternatives { choices: Vec<ItemInputStackSpec> },
}

/// Minimal target recipe IR.
///
/// This deliberately starts smaller than GTNH's full recipe vocabulary. A source
/// recipe is generated only when every behavior represented here is proven and
/// no unsupported source feature would be silently discarded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentRecipe {
    pub origin: SourceOrigin,
    pub recipe_map: ContentId,
    pub item_inputs: Vec<ItemInputSpec>,
    pub item_outputs: Vec<ItemStackSpec>,
    pub fluid_inputs: Vec<FluidStackSpec>,
    pub fluid_outputs: Vec<FluidStackSpec>,
    pub duration_ticks: u64,
    pub eut: i64,
}
