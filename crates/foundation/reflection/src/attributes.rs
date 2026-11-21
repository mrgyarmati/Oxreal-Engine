//! Editor attributes: a fixed, typed set with documented meanings (ADR-0021 §3).
//!
//! Attributes describe how tools present a type, field or variant. They never contain code and
//! never change how values are stored. [`AttributesDefinition`] is the compile-time form that
//! definitions carry; [`Attributes`] is the registered form.

use std::borrow::Cow;

/// A numeric range for editing a value: the limits and step of a slider or spin box.
///
/// It guides the UI only. The value itself isn't clamped: data from files or code may lie
/// outside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    min: f64,
    max: f64,
    step: Option<f64>,
}

impl Range {
    /// A range from `min` to `max`, inclusive. Registration checks that both are finite and
    /// that `min <= max`.
    #[must_use]
    pub const fn new(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            step: None,
        }
    }

    /// The increment for dragging and spinning. Registration checks that it's finite and
    /// positive.
    #[must_use]
    pub const fn with_step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// The lower limit.
    #[must_use]
    pub const fn min(&self) -> f64 {
        self.min
    }

    /// The upper limit.
    #[must_use]
    pub const fn max(&self) -> f64 {
        self.max
    }

    /// The increment, if one was given.
    #[must_use]
    pub const fn step(&self) -> Option<f64> {
        self.step
    }

    /// Why the range is invalid, if it is.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        if !self.min.is_finite() || !self.max.is_finite() {
            Some("range limits must be finite")
        } else if self.min > self.max {
            Some("range minimum is greater than its maximum")
        } else if self
            .step
            .is_some_and(|step| !step.is_finite() || step <= 0.0)
        {
            Some("range step must be finite and positive")
        } else {
            None
        }
    }
}

/// The physical unit of a stored value (ADR-0011: SI units, radians in code).
///
/// The editor shows the unit and converts where the conventions say so: angles are stored in
/// radians and shown in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unit {
    /// Length, `m`.
    Metres,
    /// Mass, `kg`.
    Kilograms,
    /// Time, `s`.
    Seconds,
    /// Angle, stored in radians and shown in degrees.
    Radians,
    /// Speed, `m/s`.
    MetresPerSecond,
    /// Angular speed, stored in radians per second and shown in degrees per second.
    RadiansPerSecond,
}

/// The compile-time form of [`Attributes`], built with `const` methods.
///
/// ```
/// use oxreal_reflection::{AttributesDefinition, Range, Unit};
///
/// const SPEED: AttributesDefinition = AttributesDefinition::new()
///     .display_name("Max speed")
///     .category("Movement")
///     .range(Range::new(0.0, 20.0).with_step(0.5))
///     .units(Unit::MetresPerSecond);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttributesDefinition {
    display_name: Option<&'static str>,
    category: Option<&'static str>,
    tooltip: Option<&'static str>,
    range: Option<Range>,
    units: Option<Unit>,
    hidden: bool,
    read_only: bool,
}

impl AttributesDefinition {
    /// No attributes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            display_name: None,
            category: None,
            tooltip: None,
            range: None,
            units: None,
            hidden: false,
            read_only: false,
        }
    }

    /// The label tools show instead of the Rust name.
    #[must_use]
    pub const fn display_name(mut self, display_name: &'static str) -> Self {
        self.display_name = Some(display_name);
        self
    }

    /// The group tools file it under, such as `Movement`.
    #[must_use]
    pub const fn category(mut self, category: &'static str) -> Self {
        self.category = Some(category);
        self
    }

    /// Help text. The derive defaults it to the first line of the doc comment.
    #[must_use]
    pub const fn tooltip(mut self, tooltip: &'static str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    /// The editing range.
    #[must_use]
    pub const fn range(mut self, range: Range) -> Self {
        self.range = Some(range);
        self
    }

    /// The physical unit.
    #[must_use]
    pub const fn units(mut self, units: Unit) -> Self {
        self.units = Some(units);
        self
    }

    /// Not shown by tools.
    #[must_use]
    pub const fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Shown but not editable in tools.
    #[must_use]
    pub const fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub(crate) fn to_attributes(self) -> Attributes {
        Attributes {
            display_name: self.display_name.map(Cow::Borrowed),
            category: self.category.map(Cow::Borrowed),
            tooltip: self.tooltip.map(Cow::Borrowed),
            range: self.range,
            units: self.units,
            hidden: self.hidden,
            read_only: self.read_only,
        }
    }
}

impl Default for AttributesDefinition {
    fn default() -> Self {
        Self::new()
    }
}

/// The editor attributes of a registered type, field or variant (ADR-0021 §3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attributes {
    display_name: Option<Cow<'static, str>>,
    category: Option<Cow<'static, str>>,
    tooltip: Option<Cow<'static, str>>,
    range: Option<Range>,
    units: Option<Unit>,
    hidden: bool,
    read_only: bool,
}

impl Attributes {
    /// The label tools show instead of the Rust name.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// The group tools file it under.
    #[must_use]
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    /// Help text.
    #[must_use]
    pub fn tooltip(&self) -> Option<&str> {
        self.tooltip.as_deref()
    }

    /// The editing range.
    #[must_use]
    pub fn range(&self) -> Option<Range> {
        self.range
    }

    /// The physical unit.
    #[must_use]
    pub fn units(&self) -> Option<Unit> {
        self.units
    }

    /// Whether tools hide it.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Whether tools show it without letting people edit it.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_ranges_are_reported() {
        assert_eq!(Range::new(0.0, 1.0).with_step(0.1).problem(), None);
        assert_eq!(Range::new(1.0, 1.0).problem(), None);
        assert!(Range::new(2.0, 1.0).problem().is_some());
        assert!(Range::new(f64::NAN, 1.0).problem().is_some());
        assert!(Range::new(0.0, f64::INFINITY).problem().is_some());
        assert!(Range::new(0.0, 1.0).with_step(0.0).problem().is_some());
        assert!(Range::new(0.0, 1.0).with_step(-1.0).problem().is_some());
    }

    #[test]
    fn definitions_convert_without_losing_anything() {
        let definition = AttributesDefinition::new()
            .display_name("Speed")
            .category("Movement")
            .tooltip("How fast")
            .range(Range::new(0.0, 5.0))
            .units(Unit::MetresPerSecond)
            .hidden()
            .read_only();
        let attributes = definition.to_attributes();
        assert_eq!(attributes.display_name(), Some("Speed"));
        assert_eq!(attributes.category(), Some("Movement"));
        assert_eq!(attributes.tooltip(), Some("How fast"));
        assert_eq!(attributes.range(), Some(Range::new(0.0, 5.0)));
        assert_eq!(attributes.units(), Some(Unit::MetresPerSecond));
        assert!(attributes.is_hidden() && attributes.is_read_only());
    }
}
