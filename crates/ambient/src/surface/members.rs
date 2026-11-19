//! The members of a tuple type and the parameters of a function type.

use std::cmp::Ordering;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

use super::{Path, Type};

/// Types in order, each under the name it was written with, if it was
/// written: the type of a tuple value has none.
///
/// A name is for a reader and is no part of a type, so two of these
/// that differ only in names are equal, and are ordered as one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Members<N = Path> {
    types: Vec<Type<N>>,
    /// As many as there are types, or none.
    names: Vec<String>,
}

impl<N> Members<N> {
    /// `types`, each under one of `names`.
    #[must_use]
    pub fn named(names: Vec<String>, types: Vec<Type<N>>) -> Self {
        debug_assert_eq!(names.len(), types.len());
        Self { types, names }
    }

    /// The name of the member at `index`, if it has one.
    #[must_use]
    pub fn name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }

    /// `types` under the names of these, if there are as many.
    #[must_use]
    pub fn with(&self, types: Vec<Type<N>>) -> Self {
        let names = if types.len() == self.names.len() {
            self.names.clone()
        } else {
            Vec::new()
        };
        Self { types, names }
    }

    /// What `change` makes of each type, under the name it had.
    pub fn each<M>(&self, change: impl FnMut(&Type<N>) -> Type<M>) -> Members<M> {
        Members {
            types: self.types.iter().map(change).collect(),
            names: self.names.clone(),
        }
    }

    /// As [`Members::each`], for a change that may fail.
    ///
    /// # Errors
    ///
    /// The first error `change` gives.
    pub fn try_each<M, E>(
        &self,
        change: impl FnMut(&Type<N>) -> Result<Type<M>, E>,
    ) -> Result<Members<M>, E> {
        Ok(Members {
            types: self.types.iter().map(change).collect::<Result<_, E>>()?,
            names: self.names.clone(),
        })
    }
}

impl<N> Deref for Members<N> {
    type Target = [Type<N>];

    fn deref(&self) -> &Self::Target {
        &self.types
    }
}

impl<N> From<Vec<Type<N>>> for Members<N> {
    fn from(types: Vec<Type<N>>) -> Self {
        Self {
            types,
            names: Vec::new(),
        }
    }
}

impl<N> From<Members<N>> for Vec<Type<N>> {
    fn from(members: Members<N>) -> Self {
        members.types
    }
}

impl<N> FromIterator<Type<N>> for Members<N> {
    fn from_iter<I: IntoIterator<Item = Type<N>>>(types: I) -> Self {
        Vec::from_iter(types).into()
    }
}

impl<N> IntoIterator for Members<N> {
    type Item = Type<N>;
    type IntoIter = std::vec::IntoIter<Type<N>>;

    fn into_iter(self) -> Self::IntoIter {
        self.types.into_iter()
    }
}

impl<'a, N> IntoIterator for &'a Members<N> {
    type Item = &'a Type<N>;
    type IntoIter = std::slice::Iter<'a, Type<N>>;

    fn into_iter(self) -> Self::IntoIter {
        self.types.iter()
    }
}

impl<N: PartialEq> PartialEq for Members<N> {
    fn eq(&self, other: &Self) -> bool {
        self.types == other.types
    }
}

impl<N: Eq> Eq for Members<N> {}

impl<N: PartialOrd> PartialOrd for Members<N> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.types.partial_cmp(&other.types)
    }
}

impl<N: Ord> Ord for Members<N> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.types.cmp(&other.types)
    }
}
