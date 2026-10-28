//! Source-adapter helper for dense selectors backed by Java-style affine integer state.

use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AffineBitSelectorSpec {
    pub multiplier: i32,
    pub addend: i32,
    pub pre_shift: u32,
    pub component_shifts: [u32; 3],
    pub mask: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AffineBitSelector {
    state: i32,
    spec: AffineBitSelectorSpec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffineBitSelectorError {
    ShiftOutOfRange(u32),
    NegativeMask(i32),
    UnexpectedComponentCount(usize),
    UnexpectedArgumentIndex { position: usize, index: usize },
    IntegerOutOfRange { name: &'static str, value: i64 },
    MaskMismatch { expected: i64, actual: i64 },
}

impl fmt::Display for AffineBitSelectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShiftOutOfRange(shift) => {
                write!(formatter, "selector shift {shift} exceeds i32 width")
            }
            Self::NegativeMask(mask) => write!(formatter, "selector mask {mask} is negative"),
            Self::UnexpectedComponentCount(count) => {
                write!(
                    formatter,
                    "selector has {count} components; expected exactly 3"
                )
            }
            Self::UnexpectedArgumentIndex { position, index } => write!(
                formatter,
                "selector component {position} has argument index {index}; expected {position}"
            ),
            Self::IntegerOutOfRange { name, value } => {
                write!(
                    formatter,
                    "selector {name} value {value} does not fit Java int"
                )
            }
            Self::MaskMismatch { expected, actual } => write!(
                formatter,
                "selector component mask {actual} differs from first mask {expected}"
            ),
        }
    }
}

impl Error for AffineBitSelectorError {}

impl AffineBitSelectorSpec {
    /// Build a three-component selector spec from absolute masked-shift evidence.
    ///
    /// Components are supplied as `(argument_index, absolute_shift, mask)` and must cover indices
    /// 0, 1, and 2 in order. A common leading shift is factored into `pre_shift` so nested Java
    /// arithmetic-right-shift expressions preserve their exact semantics.
    ///
    /// # Errors
    ///
    /// Rejects non-three-component shapes, out-of-order argument indices, inconsistent masks,
    /// values outside Java `int`, or shifts outside the Java `int` width.
    pub fn from_absolute_slices(
        multiplier: i64,
        addend: i64,
        slices: &[(usize, u32, i64)],
    ) -> Result<Self, AffineBitSelectorError> {
        if slices.len() != 3 {
            return Err(AffineBitSelectorError::UnexpectedComponentCount(
                slices.len(),
            ));
        }
        for (position, (argument_index, _, _)) in slices.iter().copied().enumerate() {
            if argument_index != position {
                return Err(AffineBitSelectorError::UnexpectedArgumentIndex {
                    position,
                    index: argument_index,
                });
            }
        }
        let multiplier =
            i32::try_from(multiplier).map_err(|_| AffineBitSelectorError::IntegerOutOfRange {
                name: "multiplier",
                value: multiplier,
            })?;
        let addend =
            i32::try_from(addend).map_err(|_| AffineBitSelectorError::IntegerOutOfRange {
                name: "addend",
                value: addend,
            })?;
        let mask_raw = slices[0].2;
        if slices.iter().any(|(_, _, mask)| *mask != mask_raw) {
            let actual = slices
                .iter()
                .find_map(|(_, _, mask)| (*mask != mask_raw).then_some(*mask))
                .unwrap_or(mask_raw);
            return Err(AffineBitSelectorError::MaskMismatch {
                expected: mask_raw,
                actual,
            });
        }
        let mask =
            i32::try_from(mask_raw).map_err(|_| AffineBitSelectorError::IntegerOutOfRange {
                name: "mask",
                value: mask_raw,
            })?;
        if mask < 0 {
            return Err(AffineBitSelectorError::NegativeMask(mask));
        }
        let pre_shift = slices
            .iter()
            .map(|(_, shift, _)| *shift)
            .min()
            .ok_or(AffineBitSelectorError::UnexpectedComponentCount(0))?;
        let component_shifts = [
            slices[0].1 - pre_shift,
            slices[1].1 - pre_shift,
            slices[2].1 - pre_shift,
        ];
        let spec = Self {
            multiplier,
            addend,
            pre_shift,
            component_shifts,
            mask,
        };
        AffineBitSelector::new(0, spec)?;
        Ok(spec)
    }
}

impl AffineBitSelector {
    /// Construct a selector using Java `int` wrapping arithmetic and arithmetic right shifts.
    ///
    /// # Errors
    ///
    /// Rejects shifts outside the Java `int` width and negative masks.
    pub fn new(state: i32, spec: AffineBitSelectorSpec) -> Result<Self, AffineBitSelectorError> {
        for shift in [spec.pre_shift].into_iter().chain(spec.component_shifts) {
            if shift >= i32::BITS {
                return Err(AffineBitSelectorError::ShiftOutOfRange(shift));
            }
        }
        if spec.mask < 0 {
            return Err(AffineBitSelectorError::NegativeMask(spec.mask));
        }
        Ok(Self { state, spec })
    }

    #[must_use]
    pub const fn state(&self) -> i32 {
        self.state
    }

    /// Advance one affine state step and extract three masked bit-slice components.
    #[must_use]
    pub fn next_components(&mut self) -> [i32; 3] {
        self.state = self
            .state
            .wrapping_mul(self.spec.multiplier)
            .wrapping_add(self.spec.addend);
        let sample = self.state >> self.spec.pre_shift;
        self.spec
            .component_shifts
            .map(|shift| (sample >> shift) & self.spec.mask)
    }
}

#[cfg(test)]
mod tests {
    use super::{AffineBitSelector, AffineBitSelectorSpec};

    const VANILLA_SPEC: AffineBitSelectorSpec = AffineBitSelectorSpec {
        multiplier: 3,
        addend: 1_013_904_223,
        pre_shift: 2,
        component_shifts: [0, 16, 8],
        mask: 15,
    };

    #[test]
    fn recovered_absolute_slices_build_vanilla_spec() -> Result<(), Box<dyn std::error::Error>> {
        let spec = AffineBitSelectorSpec::from_absolute_slices(
            3,
            1_013_904_223,
            &[(0, 2, 15), (1, 18, 15), (2, 10, 15)],
        )?;
        assert_eq!(spec, VANILLA_SPEC);
        Ok(())
    }

    #[test]
    fn java_int_wrapping_and_bit_slices_match_reference_sequence()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = AffineBitSelector::new(0, VANILLA_SPEC)?;
        assert_eq!(selector.next_components(), [7, 11, 12]);
        assert_eq!(selector.state(), 1_013_904_223);
        assert_eq!(selector.next_components(), [15, 14, 3]);
        assert_eq!(selector.state(), -239_350_404);
        assert_eq!(selector.next_components(), [4, 8, 6]);
        assert_eq!(selector.state(), 295_853_011);
        Ok(())
    }

    #[test]
    fn selector_does_not_deduplicate_repeated_opportunities()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = AffineBitSelector::new(81_709, VANILLA_SPEC)?;
        assert_eq!(selector.next_components(), [9, 12, 12]);
        assert_eq!(selector.next_components(), [4, 1, 1]);
        assert_eq!(selector.next_components(), [4, 1, 1]);
        Ok(())
    }
}
