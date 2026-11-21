//! Patches: validated, all-or-nothing edits of a value (ADR-0021 §4).

use crate::path::{FieldPath, ResolvedPath};
use crate::value::{OwnedValue, ValueError, ValueMut};

/// A list of edits, each a [`FieldPath`] and the new value there, applied as a whole.
///
/// [`Patch::apply`] checks every entry against the target before changing anything (Cordite's
/// validated-patch choice, ADR-0021 §4), then swaps the new values in. It returns the inverse
/// patch, holding the old values, which undo applies to restore them.
///
/// ```
/// use oxreal_reflection::{OwnedValue, Patch, Reflect, TypeRegistry};
///
/// #[derive(Reflect, Debug, PartialEq)]
/// #[reflect(id = "5d3f9b2a-7c4e-4a8f-8b6d-2e4f6a8b0c3d")]
/// struct Light {
///     intensity: f32,
///     enabled: bool,
/// }
///
/// let registry = TypeRegistry::new();
/// let mut light = Light { intensity: 1.0, enabled: false };
///
/// let mut patch = Patch::new();
/// patch.set("intensity".parse()?, OwnedValue::new(&registry, 4.0_f32)?);
/// patch.set("enabled".parse()?, OwnedValue::new(&registry, true)?);
/// let undo = patch.apply(&mut registry.view_mut(&mut light)?)?;
/// assert_eq!(light, Light { intensity: 4.0, enabled: true });
///
/// undo.apply(&mut registry.view_mut(&mut light)?)?;
/// assert_eq!(light, Light { intensity: 1.0, enabled: false });
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Default)]
pub struct Patch {
    entries: Vec<(FieldPath, OwnedValue)>,
}

impl Patch {
    /// An empty patch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry: set the value at `path` to `value`.
    pub fn set(&mut self, path: FieldPath, value: OwnedValue) -> &mut Self {
        self.entries.push((path, value));
        self
    }

    /// The entries, in the order they were added.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = (&FieldPath, &OwnedValue)> {
        self.entries.iter().map(|(path, value)| (path, value))
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the patch has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Applies every entry to `target`, or none of them, and returns the inverse patch.
    ///
    /// Before anything changes, each entry's path is resolved against `target`'s type and
    /// followed through the current value (list indices, `Option`s), and each value is checked
    /// to be of the path's type. Entries may not overlap: no entry may change a value that
    /// another entry's path is inside of, or the same value.
    ///
    /// # Errors
    ///
    /// [`PatchError::Entry`] naming the first invalid entry, or [`PatchError::Overlap`] naming
    /// two overlapping ones. `target` is unchanged then.
    ///
    /// # Panics
    ///
    /// Only on a bug in this crate: an entry that passed validation failing to apply.
    pub fn apply(self, target: &mut ValueMut<'_>) -> Result<Patch, PatchError> {
        let registry = target.as_ref().registry();
        let root = target.description().key();
        let mut resolved: Vec<ResolvedPath> = Vec::with_capacity(self.entries.len());
        for (path, value) in &self.entries {
            let entry_error = |source| PatchError::Entry {
                path: path.to_string(),
                source,
            };
            let checked = path
                .resolve(registry, root)
                .and_then(|resolved| {
                    target.as_ref().check_reachable(&resolved)?;
                    check_value(registry, &resolved, value)?;
                    Ok(resolved)
                })
                .map_err(entry_error)?;
            if let Some(other) = resolved.iter().position(|r| r.overlaps(&checked)) {
                return Err(PatchError::Overlap {
                    first: self.entries[other].0.to_string(),
                    second: path.to_string(),
                });
            }
            resolved.push(checked);
        }

        let mut inverse = Patch::new();
        for ((path, value), resolved) in self.entries.into_iter().zip(&resolved) {
            // Every entry was checked against this value, and swapping a leaf can't invalidate
            // another entry, because no entry is inside another.
            let old = target
                .replace_at(resolved, value)
                .expect("a validated, non-overlapping patch entry applies");
            inverse.set(path, old);
        }
        Ok(inverse)
    }
}

fn check_value(
    registry: &crate::TypeRegistry,
    path: &ResolvedPath,
    value: &OwnedValue,
) -> Result<(), ValueError> {
    if value.registry_id() != registry.id() {
        return Err(ValueError::WrongRegistry);
    }
    if value.description().key() != path.leaf() {
        return Err(ValueError::TypeMismatch {
            expected: registry
                .description(path.leaf())
                .map_or_else(|| path.leaf().to_string(), |d| d.name().to_owned()),
            found: value.description().name().to_owned(),
        });
    }
    Ok(())
}

/// A patch couldn't be applied. The target is unchanged.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum PatchError {
    /// An entry's path or value doesn't fit the target.
    #[error("patch entry '{path}': {source}")]
    Entry {
        /// The entry's path.
        path: String,
        /// What's wrong with it.
        #[source]
        source: ValueError,
    },
    /// Two entries overlap: one changes the value the other changes, or a value it's inside of.
    #[error("patch entries '{first}' and '{second}' overlap; each value may be changed only once")]
    Overlap {
        /// The earlier entry's path.
        first: String,
        /// The later entry's path.
        second: String,
    },
}
