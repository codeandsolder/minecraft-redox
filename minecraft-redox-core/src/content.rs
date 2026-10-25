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
    NamedVariant {
        id: ContentId,
        variant: ContentId,
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

/// Exact probability used by normalized content semantics.
///
/// Importers reduce source-specific probability scales into this rational form,
/// so the runtime does not need to know whether a source used basis points,
/// percentages, or another fixed scale.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Probability {
    pub numerator: u64,
    pub denominator: u64,
}

impl Probability {
    pub const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    #[must_use]
    pub fn new(numerator: u64, denominator: u64) -> Option<Self> {
        if denominator == 0 || numerator > denominator {
            return None;
        }
        if numerator == 0 {
            return Some(Self::ZERO);
        }
        let divisor = greatest_common_divisor(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
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
/// `amount` is how many matching items must be present. Consumption probability is
/// independent of matching, so catalysts/configuration items can be required while
/// surviving execution and probabilistic consumables remain explicit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemInputStackSpec {
    pub selector: ItemSelector,
    pub amount: u64,
    pub consumption_probability: Probability,
}

/// One concrete item production slot in a recipe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemOutputSpec {
    pub stack: ItemStackSpec,
    pub production_probability: Probability,
}

/// One recipe input slot. Alternatives occupy one slot, not several inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemInputSpec {
    Stack(ItemInputStackSpec),
    Alternatives { choices: Vec<ItemInputStackSpec> },
}

/// One source-independent requirement a machine must satisfy for a recipe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeRequirement {
    /// Minimum process heat required by the recipe, in Kelvin.
    MinimumHeat { kelvin: u64 },
    /// Minimum named machine capability required to process the recipe.
    MinimumCapability { capability: ContentId, level: u64 },
}

/// A source-independent process that grants access to a recipe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeUnlock {
    /// Consume a matching sample while scanning it for the configured time and power.
    ///
    /// Source-specific knowledge carriers (for example data sticks) are intentionally
    /// not represented here; they are transport/UI details around the resulting unlock.
    Scan {
        sample: ItemInputStackSpec,
        duration_ticks: u64,
        eut: i64,
    },
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
    pub item_outputs: Vec<ItemOutputSpec>,
    pub fluid_inputs: Vec<FluidStackSpec>,
    pub fluid_outputs: Vec<FluidStackSpec>,
    pub requirements: Vec<RecipeRequirement>,
    pub unlock: Option<RecipeUnlock>,
    pub duration_ticks: u64,
    pub eut: i64,
}
