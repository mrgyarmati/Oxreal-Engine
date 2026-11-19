//! Adapter (GPU) description and deterministic selection (ADR-0012 §6).

use std::fmt;

use crate::{AdapterRejection, RhiError};

/// Environment variable that overrides GPU selection: an adapter index (`0`, `1`, ...) or a
/// case-insensitive part of its name (`radeon`, `rtx 4080`).
pub const GPU_ENV_VAR: &str = "OXREAL_GPU";

/// A graphics API version (`major.minor.patch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl ApiVersion {
    /// A version from its parts.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What kind of adapter a GPU is. Ordered from most to least preferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterType {
    /// A dedicated graphics card.
    Discrete,
    /// A GPU integrated into the CPU package.
    Integrated,
    /// A virtualized GPU (virtual machines).
    Virtual,
    /// A software rasterizer, for example Mesa's Lavapipe.
    Cpu,
    /// Anything the driver doesn't classify.
    Other,
}

/// Describes an adapter (a GPU and its driver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    /// The adapter's name, for example `AMD Radeon RX 9070 XT`.
    pub name: String,
    /// What kind of adapter it is.
    pub adapter_type: AdapterType,
    /// PCI vendor ID.
    pub vendor_id: u32,
    /// PCI device ID.
    pub device_id: u32,
    /// The driver's name, for example `AMD proprietary driver`.
    pub driver_name: String,
    /// The driver's version string.
    pub driver_info: String,
    /// The graphics API version the device supports.
    pub api_version: ApiVersion,
}

/// One adapter offered to [`select_adapter`], with what the backend found out about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterCandidate {
    /// The adapter's description.
    pub info: AdapterInfo,
    /// Size of its largest device-local memory heap, in bytes. Used as a tie-breaker.
    pub device_local_bytes: u64,
    /// Why it can't be used. Empty when it meets every requirement.
    pub rejection_reasons: Vec<String>,
}

/// Which adapter to use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GpuSelection {
    /// Pick the best usable adapter: discrete over integrated over virtual over software, then
    /// the largest device-local memory, then the lowest index.
    #[default]
    Automatic,
    /// The adapter at this index in the driver's enumeration order.
    Index(usize),
    /// The first usable adapter whose name contains this text, ignoring case.
    NameContains(String),
}

impl GpuSelection {
    /// Parses an `OXREAL_GPU` value: a number is an index, anything else a name fragment, and an
    /// empty value means [`GpuSelection::Automatic`]. Every value is meaningful, so this can't
    /// fail.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() {
            return Self::Automatic;
        }
        match value.parse::<usize>() {
            Ok(index) => Self::Index(index),
            Err(_) => Self::NameContains(value.to_owned()),
        }
    }

    /// The selection from the `OXREAL_GPU` environment variable, or `default` if it's unset.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidEnvironment`] if the variable isn't valid Unicode.
    pub fn from_env_or(default: Self) -> Result<Self, RhiError> {
        match std::env::var(GPU_ENV_VAR) {
            Ok(value) => Ok(Self::parse(&value)),
            Err(std::env::VarError::NotPresent) => Ok(default),
            Err(std::env::VarError::NotUnicode(value)) => Err(RhiError::InvalidEnvironment {
                variable: GPU_ENV_VAR,
                value: value.to_string_lossy().into_owned(),
                expected: "an adapter index (0, 1, ...) or part of its name",
            }),
        }
    }
}

impl fmt::Display for GpuSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Automatic => f.write_str("automatic"),
            Self::Index(i) => write!(f, "adapter index {i}"),
            Self::NameContains(name) => write!(f, "name containing {name:?}"),
        }
    }
}

/// Chooses an adapter. Returns its index in `candidates`.
///
/// Deterministic: the same candidates and selection always give the same result.
///
/// # Errors
///
/// - [`RhiError::NoSuitableAdapter`] if no candidate meets the requirements. Every candidate and
///   its reasons are listed.
/// - [`RhiError::AdapterNotFound`] if an explicit selection matches no usable candidate.
pub fn select_adapter(
    candidates: &[AdapterCandidate],
    selection: &GpuSelection,
) -> Result<usize, RhiError> {
    let usable: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].rejection_reasons.is_empty())
        .collect();
    if usable.is_empty() {
        return Err(RhiError::NoSuitableAdapter {
            rejected: candidates
                .iter()
                .map(|c| AdapterRejection {
                    name: c.info.name.clone(),
                    reasons: c.rejection_reasons.clone(),
                })
                .collect(),
        });
    }

    let chosen = match selection {
        GpuSelection::Automatic => usable.iter().copied().min_by_key(|&i| {
            let c = &candidates[i];
            (
                c.info.adapter_type,
                std::cmp::Reverse(c.device_local_bytes),
                i,
            )
        }),
        GpuSelection::Index(index) => usable.iter().copied().find(|i| i == index),
        GpuSelection::NameContains(fragment) => {
            let fragment = fragment.to_lowercase();
            usable
                .iter()
                .copied()
                .find(|&i| candidates[i].info.name.to_lowercase().contains(&fragment))
        }
    };

    chosen.ok_or_else(|| RhiError::AdapterNotFound {
        selection: selection.clone(),
        available: usable
            .iter()
            .map(|&i| format!("[{i}] {}", candidates[i].info.name))
            .collect::<Vec<_>>()
            .join(", "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        name: &str,
        kind: AdapterType,
        vram_gib: u64,
        reasons: &[&str],
    ) -> AdapterCandidate {
        AdapterCandidate {
            info: AdapterInfo {
                name: name.to_owned(),
                adapter_type: kind,
                vendor_id: 0,
                device_id: 0,
                driver_name: String::new(),
                driver_info: String::new(),
                api_version: ApiVersion::new(1, 3, 0),
            },
            device_local_bytes: vram_gib << 30,
            rejection_reasons: reasons.iter().map(|r| (*r).to_owned()).collect(),
        }
    }

    #[test]
    fn automatic_prefers_discrete_then_memory_then_index() {
        let gpus = [
            candidate("llvmpipe", AdapterType::Cpu, 0, &[]),
            candidate("iGPU", AdapterType::Integrated, 2, &[]),
            candidate("Small dGPU", AdapterType::Discrete, 8, &[]),
            candidate("Big dGPU", AdapterType::Discrete, 16, &[]),
            candidate("Big dGPU twin", AdapterType::Discrete, 16, &[]),
        ];
        assert_eq!(select_adapter(&gpus, &GpuSelection::Automatic).unwrap(), 3);
    }

    #[test]
    fn rejected_adapters_are_never_chosen() {
        let gpus = [
            candidate(
                "Old dGPU",
                AdapterType::Discrete,
                24,
                &["Vulkan 1.2.0 < 1.3"],
            ),
            candidate("llvmpipe", AdapterType::Cpu, 0, &[]),
        ];
        assert_eq!(select_adapter(&gpus, &GpuSelection::Automatic).unwrap(), 1);
        let err = select_adapter(&gpus, &GpuSelection::Index(0)).unwrap_err();
        assert!(matches!(err, RhiError::AdapterNotFound { .. }), "{err}");
        assert!(err.to_string().contains("[1] llvmpipe"), "{err}");
    }

    #[test]
    fn explicit_selection_by_index_and_name() {
        let gpus = [
            candidate("Intel Arc A770", AdapterType::Discrete, 16, &[]),
            candidate("AMD Radeon RX 9070 XT", AdapterType::Discrete, 16, &[]),
        ];
        assert_eq!(select_adapter(&gpus, &GpuSelection::Index(1)).unwrap(), 1);
        let by_name = GpuSelection::NameContains("radeon".into());
        assert_eq!(select_adapter(&gpus, &by_name).unwrap(), 1);
        let missing = GpuSelection::NameContains("geforce".into());
        assert!(select_adapter(&gpus, &missing).is_err());
    }

    #[test]
    fn no_usable_adapter_reports_all_reasons() {
        let gpus = [candidate(
            "Old",
            AdapterType::Discrete,
            4,
            &["missing feature x"],
        )];
        match select_adapter(&gpus, &GpuSelection::Automatic).unwrap_err() {
            RhiError::NoSuitableAdapter { rejected } => {
                assert_eq!(rejected.len(), 1);
                assert_eq!(rejected[0].reasons, vec!["missing feature x".to_owned()]);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(select_adapter(&[], &GpuSelection::Automatic).is_err());
    }

    #[test]
    fn parses_gpu_selection_values() {
        assert_eq!(GpuSelection::parse(""), GpuSelection::Automatic);
        assert_eq!(GpuSelection::parse(" 1 "), GpuSelection::Index(1));
        assert_eq!(
            GpuSelection::parse("RX 9070"),
            GpuSelection::NameContains("RX 9070".into())
        );
    }

    #[test]
    fn api_versions_order_and_display() {
        assert!(ApiVersion::new(1, 3, 0) > ApiVersion::new(1, 2, 999));
        assert_eq!(ApiVersion::new(1, 4, 359).to_string(), "1.4.359");
    }
}
