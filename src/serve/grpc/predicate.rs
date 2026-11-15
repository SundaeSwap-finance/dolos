//! The clause rules of a u5c predicate, for every predicate type and every
//! RPC version.

/// What a predicate admits, as the clause rules combine it.
pub(super) trait Admitted: Sized {
    /// Admits everything.
    fn all() -> Self;

    /// Admits nothing.
    fn none() -> Self;

    /// Admits what both admit.
    fn intersection(self, other: Self) -> Self;

    /// Admits what either admits.
    fn union(self, other: Self) -> Self;

    /// Admits what this does not admit.
    fn complement(self) -> Self;
}

impl Admitted for bool {
    fn all() -> Self {
        true
    }

    fn none() -> Self {
        false
    }

    fn intersection(self, other: Self) -> Self {
        self && other
    }

    fn union(self, other: Self) -> Self {
        self || other
    }

    fn complement(self) -> Self {
        !self
    }
}

/// The clauses of a predicate.
pub(super) struct Clauses<P: Predicate> {
    pub(super) matching: Option<P::Pattern>,
    pub(super) not: Vec<P>,
    pub(super) all_of: Vec<P>,
    pub(super) any_of: Vec<P>,
}

/// A predicate of one RPC version.
pub(super) trait Predicate: Sized {
    /// The pattern of the match clause.
    type Pattern;

    /// The clauses of the predicate.
    fn into_clauses(self) -> Clauses<Self>;
}

/// What `predicate` admits, with `matched` giving what a match pattern admits.
pub(super) fn evaluate<P: Predicate, A: Admitted, E>(
    predicate: P,
    matched: &mut impl FnMut(P::Pattern) -> Result<A, E>,
) -> Result<A, E> {
    let Clauses {
        matching,
        not,
        all_of,
        any_of,
    } = predicate.into_clauses();

    let mut admitted = match matching {
        Some(pattern) => matched(pattern)?,
        None => A::all(),
    };
    for x in not {
        admitted = admitted.intersection(evaluate(x, matched)?.complement());
    }
    for x in all_of {
        admitted = admitted.intersection(evaluate(x, matched)?);
    }
    if !any_of.is_empty() {
        let mut either = A::none();
        for x in any_of {
            either = either.union(evaluate(x, matched)?);
        }
        admitted = admitted.intersection(either);
    }

    Ok(admitted)
}
