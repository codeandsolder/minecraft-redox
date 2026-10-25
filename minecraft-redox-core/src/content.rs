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
    /// Resolve to the first identity that exists in the loaded content registry.
    ///
    /// This is an ordered identity fallback, not a recipe-input alternative: once the
    /// preferred identity exists, later choices are not accepted as substitutes.
    FirstAvailable {
        choices: Vec<FluidSelector>,
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

/// One fluid input slot. Alternatives can require different amounts per substitute.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FluidInputSpec {
    Stack(FluidStackSpec),
    Alternatives { choices: Vec<FluidStackSpec> },
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

/// A relationship that must hold over the machine input inventory when a recipe starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeInputConstraint {
    /// Aggregate available fluid matching the two recipe input slots must have this exact ratio.
    ///
    /// The equality is `left_amount * right_units == right_amount * left_units`.
    ExactFluidInventoryRatio {
        left_input: usize,
        right_input: usize,
        left_units: u64,
        right_units: u64,
    },
}

/// One source-independent requirement a machine must satisfy for a recipe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeRequirement {
    /// Minimum process heat required by the recipe, in Kelvin.
    MinimumHeat { kelvin: u64 },
    /// Minimum named machine capability required to process the recipe.
    MinimumCapability { capability: ContentId, level: u64 },
    /// Exact named machine capability value required to select this recipe.
    ExactCapability { capability: ContentId, level: u64 },
    /// Named runtime environment in which the process must execute.
    ///
    /// Environment quality and machine-specific effects of that quality are runtime concerns,
    /// not fixed recipe constants.
    RequiredEnvironment { environment: ContentId },
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

/// One named runtime resource whose consumption drives process progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgressResourceSpec {
    pub resource: ContentId,
    pub amount: u64,
}

/// Energy semantics for one consumed item fuel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemFuelSpec {
    /// Total electrical energy made available by consuming one item, in EU.
    pub total_energy_eu: u64,
    /// Electrical output rate while that stored fuel energy is being emitted, in EU/t.
    pub output_eut: u64,
}

/// What advances an executable recipe toward completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeProgress {
    /// Progress advances with wall-clock machine ticks.
    Timed { duration_ticks: u64 },
    /// Progress advances only by consuming the listed runtime resources.
    ///
    /// The process completes once every required amount has been consumed; resources may
    /// arrive incrementally while the recipe remains active.
    ResourceDriven {
        resources: Vec<ProgressResourceSpec>,
    },
}

/// A source-independent condition controlling whether recipe content is available at all.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeAvailabilityRequirement {
    /// A named content module must be present before this recipe is registered.
    RequiredContentModule { module: ContentId },
}

/// Whether a recipe participates in executable process lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeExecutionMode {
    Executable,
    /// Retained for recipe browsers, documentation, or other reference surfaces, but never run.
    ReferenceOnly,
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
    pub availability_requirements: Vec<RecipeAvailabilityRequirement>,
    pub execution_mode: RecipeExecutionMode,
    pub item_inputs: Vec<ItemInputSpec>,
    pub item_outputs: Vec<ItemOutputSpec>,
    pub fluid_inputs: Vec<FluidInputSpec>,
    pub fluid_outputs: Vec<FluidStackSpec>,
    pub input_constraints: Vec<RecipeInputConstraint>,
    pub requirements: Vec<RecipeRequirement>,
    pub unlock: Option<RecipeUnlock>,
    /// One-time energy charged when the recipe starts, in EU.
    pub startup_energy_eu: u64,
    /// Fuel energy density, in EU per litre, when this entry defines a fluid generator fuel.
    pub fuel_energy_eu_per_liter: Option<u64>,
    /// Total-energy/output-rate semantics when consuming one item as generator fuel.
    pub item_fuel: Option<ItemFuelSpec>,
    pub progress: RecipeProgress,
    pub eut: i64,
}
