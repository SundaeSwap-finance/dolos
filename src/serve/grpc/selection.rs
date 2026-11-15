//! The outputs a UTxO predicate admits, built from the outputs each of its
//! match patterns admits.
use dolos_core::UtxoSet;
use tonic::Status;

use super::predicate::Admitted;

/// The outputs a predicate admits, held as a set of references or as every
/// reference outside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Selection {
    /// The outputs of the set.
    Only(UtxoSet),
    /// Every output outside the set.
    AllExcept(UtxoSet),
}

impl Admitted for Selection {
    fn all() -> Self {
        Self::AllExcept(UtxoSet::new())
    }

    fn none() -> Self {
        Self::Only(UtxoSet::new())
    }

    fn intersection(self, other: Self) -> Self {
        match (self, other) {
            (Self::Only(a), Self::Only(b)) => Self::Only(&a & &b),
            (Self::Only(a), Self::AllExcept(b)) | (Self::AllExcept(b), Self::Only(a)) => {
                Self::Only(&a - &b)
            }
            (Self::AllExcept(a), Self::AllExcept(b)) => Self::AllExcept(&a | &b),
        }
    }

    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Only(a), Self::Only(b)) => Self::Only(&a | &b),
            (Self::Only(a), Self::AllExcept(b)) | (Self::AllExcept(b), Self::Only(a)) => {
                Self::AllExcept(&b - &a)
            }
            (Self::AllExcept(a), Self::AllExcept(b)) => Self::AllExcept(&a & &b),
        }
    }

    fn complement(self) -> Self {
        match self {
            Self::Only(x) => Self::AllExcept(x),
            Self::AllExcept(x) => Self::Only(x),
        }
    }
}

impl Selection {
    /// The references of the outputs the selection admits, refused when it
    /// admits every output outside a set.
    pub(super) fn into_bounded(self) -> Result<UtxoSet, Status> {
        match self {
            Self::Only(x) => Ok(x),
            Self::AllExcept(_) => Err(Status::invalid_argument(
                "criteria too broad, narrow it down",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dolos_core::TxoRef;
    use pallas::crypto::hash::Hash;

    /// The set of the references numbered `held`.
    fn set(held: &[u8]) -> UtxoSet {
        held.iter()
            .map(|i| TxoRef(Hash::new([*i; 32]), 0))
            .collect()
    }

    /// The selection of exactly the references numbered `held`.
    fn only(held: &[u8]) -> Selection {
        Selection::Only(set(held))
    }

    /// The selection of every reference except those numbered `held`.
    fn all_except(held: &[u8]) -> Selection {
        Selection::AllExcept(set(held))
    }

    #[test]
    fn each_operation_admits_what_the_set_operation_on_its_operands_admits() {
        for (got, expected) in [
            (only(&[1, 2]).intersection(only(&[2, 3])), only(&[2])),
            (only(&[1, 2]).intersection(all_except(&[2, 3])), only(&[1])),
            (all_except(&[2, 3]).intersection(only(&[1, 2])), only(&[1])),
            (
                all_except(&[1, 2]).intersection(all_except(&[2, 3])),
                all_except(&[1, 2, 3]),
            ),
            (only(&[1, 2]).union(only(&[2, 3])), only(&[1, 2, 3])),
            (only(&[1, 2]).union(all_except(&[2, 3])), all_except(&[3])),
            (all_except(&[2, 3]).union(only(&[1, 2])), all_except(&[3])),
            (
                all_except(&[1, 2]).union(all_except(&[2, 3])),
                all_except(&[2]),
            ),
            (only(&[1]).complement(), all_except(&[1])),
            (all_except(&[1]).complement(), only(&[1])),
            (Selection::all().complement(), Selection::none()),
        ] {
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn only_a_selection_of_named_outputs_is_bounded() {
        assert_eq!(
            all_except(&[1]).into_bounded().unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            Selection::all().into_bounded().unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(only(&[1, 2]).into_bounded().unwrap(), set(&[1, 2]));
        assert_eq!(Selection::none().into_bounded().unwrap(), set(&[]));
    }
}
