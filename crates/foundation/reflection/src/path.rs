//! Field paths: where a value sits inside another, by name for files and resolved for access
//! (ADR-0021 §4).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use crate::description::{TypeDescription, TypeKey, TypeKind};
use crate::registry::{RegistryId, TypeRegistry};
use crate::value::ValueError;

/// Where a value sits inside another, by name: the persistent form, which overrides and saved
/// data use (ADR-0006, D-07).
///
/// Written as text like `movement.waypoints[2].x` or `target?.health`:
///
/// - `.name` (or `name` first) selects a struct field, by its name or a former name (alias), or
///   a math type's component such as `x`. Tuple-struct fields are named `0`, `1` and so on.
/// - `[n]` selects an element of an array or list.
/// - `?` selects the value inside an `Option`.
///
/// An empty path is the value itself. Resolve a path against a type with
/// [`FieldPath::resolve`] before using it.
///
/// ```
/// use oxreal_reflection::FieldPath;
///
/// let path: FieldPath = "movement.waypoints[2].x".parse()?;
/// assert_eq!(path, FieldPath::new().field("movement").field("waypoints").index(2).field("x"));
/// assert_eq!(path.to_string(), "movement.waypoints[2].x");
/// # Ok::<(), oxreal_reflection::ParseFieldPathError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct FieldPath {
    segments: Vec<PathSegment>,
}

/// One step of a [`FieldPath`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathSegment {
    /// A struct field or math component, by name.
    Field(Cow<'static, str>),
    /// An array or list element.
    Index(usize),
    /// The value inside an `Option`.
    Some,
}

impl FieldPath {
    /// The empty path: the value itself.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// This path, then the field or component `name`.
    #[must_use]
    pub fn field(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.segments.push(PathSegment::Field(name.into()));
        self
    }

    /// This path, then the element at `index`.
    #[must_use]
    pub fn index(mut self, index: usize) -> Self {
        self.segments.push(PathSegment::Index(index));
        self
    }

    /// This path, then the value inside an `Option`.
    #[must_use]
    pub fn some(mut self) -> Self {
        self.segments.push(PathSegment::Some);
        self
    }

    /// The segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    /// Whether this is the empty path, the value itself.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Resolves the path against the type `root` in `registry`, checking every segment against
    /// the descriptions. Array indices are checked here; list indices and `Option` values are
    /// checked when the path is used, because they depend on the value.
    ///
    /// # Errors
    ///
    /// A [`ValueError`] naming the segment's type and what's wrong: an unknown field, a segment
    /// that doesn't fit the kind (an index into a struct), an opaque field, an array index out of
    /// bounds, or a segment after a math component other than another component.
    pub fn resolve(
        &self,
        registry: &TypeRegistry,
        root: TypeKey,
    ) -> Result<ResolvedPath, ValueError> {
        let mut current = registry
            .description(root)
            .ok_or(ValueError::UnknownType { key: root })?;
        let mut steps = Vec::new();
        let mut components = Vec::new();
        for segment in &self.segments {
            let in_math = !components.is_empty();
            let (step, next) = match (segment, current.kind()) {
                (PathSegment::Field(name), TypeKind::Math(info)) => {
                    let index = info
                        .components()
                        .iter()
                        .position(|c| c.name() == name)
                        .ok_or_else(|| no_such_field(&current, name))?;
                    let ty = info.components()[index].ty();
                    components.push((index, ty));
                    current = description(registry, ty)?;
                    continue;
                }
                _ if in_math => {
                    return Err(ValueError::AfterMathComponent {
                        ty: current.name().to_owned(),
                    });
                }
                (PathSegment::Field(name), TypeKind::Struct(info)) => {
                    let (index, field) = info
                        .field(name)
                        .ok_or_else(|| no_such_field(&current, name))?;
                    let ty = field.ty().ok_or_else(|| ValueError::OpaqueField {
                        ty: current.name().to_owned(),
                        field: field.name().to_owned(),
                    })?;
                    (
                        Step::Field {
                            index,
                            offset: field.offset(),
                        },
                        ty,
                    )
                }
                (PathSegment::Index(index), TypeKind::Array(info)) => {
                    if *index >= info.len() {
                        return Err(ValueError::IndexOutOfBounds {
                            ty: current.name().to_owned(),
                            index: *index,
                            len: info.len(),
                        });
                    }
                    let element = description(registry, info.element())?;
                    (
                        Step::ArrayElement {
                            offset: index * element.layout().size(),
                        },
                        info.element(),
                    )
                }
                (PathSegment::Index(index), TypeKind::List(info)) => {
                    (Step::ListElement { index: *index }, info.element())
                }
                (PathSegment::Some, TypeKind::Option(info)) => (Step::Some, info.some()),
                (segment, _) => {
                    return Err(ValueError::WrongKind {
                        ty: current.name().to_owned(),
                        expected: match segment {
                            PathSegment::Field(_) => "a struct or math type",
                            PathSegment::Index(_) => "an array or list",
                            PathSegment::Some => "an option",
                        },
                    });
                }
            };
            steps.push(ResolvedStep {
                container: current.key(),
                step,
                ty: next,
            });
            current = description(registry, next)?;
        }
        Ok(ResolvedPath {
            registry: registry.id(),
            root,
            leaf: current.key(),
            steps,
            components,
        })
    }
}

fn description(
    registry: &TypeRegistry,
    key: TypeKey,
) -> Result<std::sync::Arc<TypeDescription>, ValueError> {
    registry
        .description(key)
        .ok_or(ValueError::UnknownType { key })
}

fn no_such_field(ty: &TypeDescription, field: &str) -> ValueError {
    ValueError::NoSuchField {
        ty: ty.name().to_owned(),
        field: field.to_owned(),
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.segments.iter().enumerate() {
            match segment {
                PathSegment::Field(name) if i == 0 => f.write_str(name)?,
                PathSegment::Field(name) => write!(f, ".{name}")?,
                PathSegment::Index(index) => write!(f, "[{index}]")?,
                PathSegment::Some => f.write_str("?")?,
            }
        }
        Ok(())
    }
}

impl FromStr for FieldPath {
    type Err = ParseFieldPathError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = |position: usize, problem: &'static str| ParseFieldPathError {
            path: text.to_owned(),
            position,
            problem,
        };
        let is_name_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let mut path = Self::new();
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            let position = text.len() - rest.len();
            match c {
                '.' if path.is_empty() => {
                    return Err(error(position, "a path can't start with '.'"));
                }
                '.' => {
                    let name_len = rest[1..]
                        .find(|c| !is_name_char(c))
                        .unwrap_or(rest.len() - 1);
                    if name_len == 0 {
                        return Err(error(position + 1, "expected a field name after '.'"));
                    }
                    path = path.field(rest[1..=name_len].to_owned());
                    rest = &rest[1 + name_len..];
                }
                '[' => {
                    let close = rest
                        .find(']')
                        .ok_or_else(|| error(position, "'[' without a closing ']'"))?;
                    let index = rest[1..close]
                        .parse::<usize>()
                        .map_err(|_| error(position + 1, "expected an index such as [0]"))?;
                    path = path.index(index);
                    rest = &rest[close + 1..];
                }
                '?' => {
                    path = path.some();
                    rest = &rest[1..];
                }
                c if is_name_char(c) && path.is_empty() => {
                    let name_len = rest.find(|c| !is_name_char(c)).unwrap_or(rest.len());
                    path = path.field(rest[..name_len].to_owned());
                    rest = &rest[name_len..];
                }
                _ => return Err(error(position, "expected '.', '[' or '?'")),
            }
        }
        Ok(path)
    }
}

/// Text that isn't a field path was parsed as one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("'{path}' is not a field path (at byte {position}): {problem}")]
pub struct ParseFieldPathError {
    path: String,
    position: usize,
    problem: &'static str,
}

/// A [`FieldPath`] resolved against one type in one registry: the runtime form, with offsets
/// and indices instead of names (ADR-0021 §4). Valid for this build and registry only; never
/// save it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub(crate) registry: RegistryId,
    pub(crate) root: TypeKey,
    pub(crate) leaf: TypeKey,
    pub(crate) steps: Vec<ResolvedStep>,
    /// Trailing math components: (component index, component type).
    pub(crate) components: Vec<(usize, TypeKey)>,
}

impl ResolvedPath {
    /// The type the path starts at.
    #[must_use]
    pub fn root(&self) -> TypeKey {
        self.root
    }

    /// The type of the value the path reaches.
    #[must_use]
    pub fn leaf(&self) -> TypeKey {
        self.leaf
    }

    /// Whether the path ends inside a math value (`translation.x`). Such a value is copied in
    /// and out rather than borrowed, because glam's SIMD types hide their fields.
    #[must_use]
    pub fn ends_in_math_component(&self) -> bool {
        !self.components.is_empty()
    }

    /// Whether `self` and `other` reach the same value, or one reaches a value inside the
    /// other's.
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        let shared = self.steps.len().min(other.steps.len());
        if self.steps[..shared] != other.steps[..shared] {
            return false;
        }
        if self.steps.len() != other.steps.len() {
            return true;
        }
        let shared = self.components.len().min(other.components.len());
        self.components[..shared] == other.components[..shared]
    }
}

/// One resolved step: from a value of type `container` to the inner value of type `ty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedStep {
    pub(crate) container: TypeKey,
    pub(crate) step: Step,
    pub(crate) ty: TypeKey,
}

/// How to reach the inner value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A struct field, by index (zero-sized fields can share an offset) and byte offset.
    Field { index: usize, offset: usize },
    /// An array element, at a byte offset (already checked against the length).
    ArrayElement { offset: usize },
    /// A list element (checked against the length when used).
    ListElement { index: usize },
    /// An option's payload (checked for `Some` when used).
    Some,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip_through_text() {
        for text in [
            "",
            "a",
            "a.b",
            "items[3]",
            "target?.health",
            "pair.0",
            "m.x_axis.y",
            "a[0][1]?",
        ] {
            let path: FieldPath = text.parse().expect(text);
            assert_eq!(path.to_string(), text);
        }
        let path: FieldPath = "target?.waypoints[2].x".parse().expect("valid");
        assert_eq!(
            path.segments(),
            [
                PathSegment::Field("target".into()),
                PathSegment::Some,
                PathSegment::Field("waypoints".into()),
                PathSegment::Index(2),
                PathSegment::Field("x".into()),
            ]
        );
    }

    #[test]
    fn malformed_paths_are_errors_naming_the_position() {
        for (text, position) in [
            (".a", 0),
            ("a.", 2),
            ("a..b", 2),
            ("a[", 1),
            ("a[x]", 2),
            ("a[-1]", 2),
            ("a b", 1),
            ("a-b", 1),
        ] {
            let error = text.parse::<FieldPath>().expect_err(text);
            assert_eq!(error.position, position, "{text}: {error}");
            assert!(error.to_string().contains(&format!("'{text}'")), "{error}");
        }
    }
}
