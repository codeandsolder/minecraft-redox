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

/// One recipe fluid-input slot. Alternatives occupy one slot, not several inputs.
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

/// One recipe input slot. Alternatives occupy one slot, not several inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemInputSpec {
    Stack(ItemStackSpec),
    Alternatives { choices: Vec<ItemStackSpec> },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeCondition {
    CleanRoom,
    LowGravity,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeBehavior {
    Fake,
    Hidden,
    Disabled,
    IgnoreCollision,
    NbtSensitive,
    NoBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecipeSlotChance {
    pub slot: u32,
    /// Chance on GT scale 0..=10000, where 10000 is 100%.
    pub chance_10000: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeSpecialSpec {
    ItemStack { value: ItemStackSpec },
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecipePolicySpec {
    pub conditions: Vec<RecipeCondition>,
    pub behaviors: Vec<RecipeBehavior>,
    pub required_mods: Vec<ContentId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemStackIntegerPair {
    pub item: ItemStackSpec,
    pub value: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipeMetadataValue {
    Integer {
        value: i64,
    },
    Decimal {
        value: FixedDecimal,
    },
    DecimalTriple {
        first: FixedDecimal,
        second: FixedDecimal,
        third: FixedDecimal,
    },
    Boolean {
        value: bool,
    },
    Text {
        value: String,
    },
    ContentRef {
        id: ContentId,
    },
    ItemStack {
        value: ItemStackSpec,
    },
    ItemStacks {
        values: Vec<ItemStackSpec>,
    },
    ItemStackIntegerPairs {
        values: Vec<ItemStackIntegerPair>,
    },
    Scanning {
        duration_ticks: u64,
        voltage: i64,
    },
    Sievert {
        value: i64,
        exact: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecipeMetadataSpec {
    pub key: ContentId,
    pub value: RecipeMetadataValue,
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
    pub fluid_inputs: Vec<FluidInputSpec>,
    pub fluid_outputs: Vec<FluidStackSpec>,
    pub duration_ticks: u64,
    pub eut: i64,
    /// Explicit input-consumption chances. Omitted slots consume at 100%.
    pub item_input_consumption_chances: Vec<RecipeSlotChance>,
    /// Explicit output-production chances. Omitted slots produce at 100%.
    pub item_output_production_chances: Vec<RecipeSlotChance>,
    pub metadata: Vec<RecipeMetadataSpec>,
    pub category: Option<ContentId>,
    pub special: Option<RecipeSpecialSpec>,
    pub special_value: Option<i64>,
    pub policy: RecipePolicySpec,
}

/// Coarse material form groups generated together by the legacy material builder.
///
/// Keeping groups intact preserves the source intent without baking GTNH's exact
/// `OrePrefix` expansion table into the archaeological IR.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialFormGroup {
    Dust,
    Metal,
    Gem,
    Ore,
    Cell,
    Plasma,
    ToolHead,
    Gear,
    Empty,
    Fluid,
    Gas,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterialColorSpec {
    Argb { value: u32 },
    Named { id: ContentId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentTextSpec {
    Literal { value: String },
    Symbol { id: ContentId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterialIconSetSpec {
    Reference {
        id: ContentId,
    },
    CustomTextures {
        base: ContentId,
        namespace: ContentTextSpec,
        name: Option<ContentTextSpec>,
        textures: Vec<ContentId>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialComponentSpec {
    pub material: ContentId,
    pub amount: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialAspectSpec {
    pub aspect: ContentId,
    pub amount: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FixedDecimal {
    pub coefficient: i64,
    pub decimal_places: u32,
}

impl FixedDecimal {
    #[must_use]
    pub const fn integer(value: i64) -> Self {
        Self {
            coefficient: value,
            decimal_places: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialToolSpec {
    pub durability: i64,
    pub quality: i64,
    pub speed: FixedDecimal,
}

impl Default for MaterialToolSpec {
    fn default() -> Self {
        Self {
            durability: 0,
            quality: 0,
            speed: FixedDecimal::integer(1),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialTurbineSpec {
    pub steam_multiplier: FixedDecimal,
    pub gas_multiplier: FixedDecimal,
    pub plasma_multiplier: FixedDecimal,
}

impl Default for MaterialTurbineSpec {
    fn default() -> Self {
        Self {
            steam_multiplier: FixedDecimal::integer(1),
            gas_multiplier: FixedDecimal::integer(1),
            plasma_multiplier: FixedDecimal::integer(1),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterialFormulaPart {
    Text { value: String },
    Glyph { name: String },
    Formatting { name: String },
    MaterialFormula { material: ContentId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialChemicalFormulaSpec {
    pub parts: Vec<MaterialFormulaPart>,
    pub localized: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialDensitySpec {
    pub multiplier: i64,
    pub divider: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialFuelSpec {
    pub fuel_type: ContentId,
    pub power: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialEnchantmentSpec {
    pub enchantment: ContentId,
    pub level: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialArcSmeltingWithGasSpec {
    pub gas: ContentId,
    pub material: ContentId,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialProcessingBehavior {
    ElectrolyzerRecipe,
    CentrifugeRecipe,
    CrackingRecipes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialAutoRecipePolicy {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialProcessingSpec {
    pub generated_behaviors: Vec<MaterialProcessingBehavior>,
    pub blast_furnace: MaterialAutoRecipePolicy,
    pub vacuum_freezer: MaterialAutoRecipePolicy,
    pub recycling: MaterialAutoRecipePolicy,
}

impl Default for MaterialProcessingSpec {
    fn default() -> Self {
        Self {
            generated_behaviors: Vec::new(),
            blast_furnace: MaterialAutoRecipePolicy::Enabled,
            vacuum_freezer: MaterialAutoRecipePolicy::Enabled,
            recycling: MaterialAutoRecipePolicy::Enabled,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentMaterial {
    pub origin: SourceOrigin,
    pub name: String,
    pub default_local_name: String,
    pub icon_set: Option<MaterialIconSetSpec>,
    pub color: Option<MaterialColorSpec>,
    pub melting_point: Option<i64>,
    pub blast_furnace_temp: Option<i64>,
    pub form_groups: Vec<MaterialFormGroup>,
    pub added_forms: Vec<ContentId>,
    pub removed_forms: Vec<ContentId>,
    pub tags: Vec<ContentId>,
    pub components: Vec<MaterialComponentSpec>,
    pub aspects: Vec<MaterialAspectSpec>,
    pub ore_byproducts: Vec<ContentId>,
    pub element: Option<ContentId>,
    pub mining_level: Option<i64>,
    pub ore_multiplier: Option<i64>,
    pub blast_furnace_required: Option<bool>,
    pub glowing_ore: bool,
    pub unifiable: bool,
    pub molten_argb: u32,
    pub density: MaterialDensitySpec,
    pub fuel: MaterialFuelSpec,
    pub tool: MaterialToolSpec,
    pub turbine: MaterialTurbineSpec,
    pub heat_damage: FixedDecimal,
    pub chemical_formula: Option<MaterialChemicalFormulaSpec>,
    pub tool_enchantment: Option<MaterialEnchantmentSpec>,
    pub armor_enchantment: Option<MaterialEnchantmentSpec>,
    pub processing: MaterialProcessingSpec,
    pub direct_smelting: Option<ContentId>,
    pub smelting_into: Option<ContentId>,
    pub macerating_into: Option<ContentId>,
    pub arc_smelting_into: Option<ContentId>,
    pub arc_smelting_with_gas: Vec<MaterialArcSmeltingWithGasSpec>,
    pub flavor_text: Option<String>,
    pub alias_key: String,
}
