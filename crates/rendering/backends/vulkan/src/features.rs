//! Oxreal's required Vulkan device features (ADR-0012, ADR-0016 §3).
//!
//! [`REQUIRED`] is the list enforced at device selection. A test keeps it identical to
//! `tools/ci/required-vulkan-features.txt`, which CI checks against Lavapipe, so the documented,
//! CI-verified and enforced lists can never drift apart.

use ash::vk;

/// The feature structures Oxreal queries and enables. `p_next` is always null here: chains are
/// built only for the duration of a query or of device creation.
#[derive(Default)]
pub(crate) struct FeatureSet {
    pub(crate) core: vk::PhysicalDeviceFeatures,
    pub(crate) v12: vk::PhysicalDeviceVulkan12Features<'static>,
    pub(crate) v13: vk::PhysicalDeviceVulkan13Features<'static>,
}

/// One required feature: its Vulkan name, how to read it and how to request it.
pub(crate) struct RequiredFeature {
    pub(crate) name: &'static str,
    supported: fn(&FeatureSet) -> bool,
    enable: fn(&mut FeatureSet),
}

macro_rules! required {
    ($name:literal, $part:ident . $field:ident) => {
        RequiredFeature {
            name: $name,
            supported: |f| f.$part.$field == vk::TRUE,
            enable: |f| f.$part.$field = vk::TRUE,
        }
    };
}

/// Every feature Oxreal requires, in the same order as `required-vulkan-features.txt`.
pub(crate) static REQUIRED: &[RequiredFeature] = &[
    // Vulkan 1.3 core features the renderer relies on.
    required!("dynamicRendering", v13.dynamic_rendering),
    required!("synchronization2", v13.synchronization2),
    required!("maintenance4", v13.maintenance4),
    required!("bufferDeviceAddress", v12.buffer_device_address),
    required!("timelineSemaphore", v12.timeline_semaphore),
    // Bindless (Roadmap 2022 subset, ADR-0016).
    required!("descriptorIndexing", v12.descriptor_indexing),
    required!("runtimeDescriptorArray", v12.runtime_descriptor_array),
    required!(
        "descriptorBindingPartiallyBound",
        v12.descriptor_binding_partially_bound
    ),
    required!(
        "descriptorBindingVariableDescriptorCount",
        v12.descriptor_binding_variable_descriptor_count
    ),
    required!(
        "descriptorBindingSampledImageUpdateAfterBind",
        v12.descriptor_binding_sampled_image_update_after_bind
    ),
    required!(
        "descriptorBindingStorageImageUpdateAfterBind",
        v12.descriptor_binding_storage_image_update_after_bind
    ),
    required!(
        "descriptorBindingUpdateUnusedWhilePending",
        v12.descriptor_binding_update_unused_while_pending
    ),
    required!(
        "shaderSampledImageArrayNonUniformIndexing",
        v12.shader_sampled_image_array_non_uniform_indexing
    ),
    required!(
        "shaderStorageImageArrayNonUniformIndexing",
        v12.shader_storage_image_array_non_uniform_indexing
    ),
    required!("scalarBlockLayout", v12.scalar_block_layout),
    // Baseline quality.
    required!("samplerAnisotropy", core.sampler_anisotropy),
];

/// Names of the required features that `available` lacks, in list order.
pub(crate) fn missing(available: &FeatureSet) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .filter(|feature| !(feature.supported)(available))
        .map(|feature| feature.name)
        .collect()
}

/// A feature set with exactly the required features enabled and nothing else.
pub(crate) fn required_set() -> FeatureSet {
    let mut set = FeatureSet::default();
    for feature in REQUIRED {
        (feature.enable)(&mut set);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CI-checked list (ADR-0016 §3). Path is relative to this source file.
    const CI_LIST: &str = include_str!("../../../../../tools/ci/required-vulkan-features.txt");

    fn ci_names() -> Vec<&'static str> {
        CI_LIST
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
            .collect()
    }

    #[test]
    fn enforced_list_matches_the_ci_checked_list_exactly() {
        let ours: Vec<&str> = REQUIRED.iter().map(|f| f.name).collect();
        assert_eq!(
            ours,
            ci_names(),
            "features.rs and tools/ci/required-vulkan-features.txt must list the same features \
             in the same order"
        );
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = REQUIRED.iter().map(|f| f.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), REQUIRED.len());
    }

    #[test]
    fn required_set_satisfies_every_requirement() {
        assert!(missing(&required_set()).is_empty());
    }

    #[test]
    fn an_empty_set_misses_everything_in_order() {
        let all: Vec<&str> = REQUIRED.iter().map(|f| f.name).collect();
        assert_eq!(missing(&FeatureSet::default()), all);
    }

    #[test]
    fn each_feature_is_checked_independently() {
        // Turning one feature off must report exactly that feature. This catches a getter and
        // setter that point at different fields.
        for (i, feature) in REQUIRED.iter().enumerate() {
            let mut set = FeatureSet::default();
            for (j, other) in REQUIRED.iter().enumerate() {
                if i != j {
                    (other.enable)(&mut set);
                }
            }
            assert_eq!(missing(&set), vec![feature.name]);
        }
    }
}
