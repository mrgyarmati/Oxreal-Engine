//! The Vulkan loader, instance, validation layers and debug messenger.

use std::ffi::{CStr, CString};

use ash::prelude::VkResult;
use ash::vk;
use oxreal_diagnostics::{debug, info, warn};
use oxreal_rhi::{RhiError, Validation};

use crate::debug::{DebugState, messenger_create_info};
use crate::error::from_vk;

/// Environment variable that forces validation layers on (`1`) or off (`0`), overriding
/// [`oxreal_rhi::DeviceDesc::validation`].
pub const VALIDATION_ENV_VAR: &str = "OXREAL_VULKAN_VALIDATION";

/// Environment variable that turns on synchronization validation (`1`): missing or wrong
/// barriers become validation errors. It needs validation on, and instance creation fails if
/// the layer can't provide it, so a run that asked for it never silently goes without.
pub const SYNC_VALIDATION_ENV_VAR: &str = "OXREAL_VULKAN_SYNC_VALIDATION";

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";
const MIN_API_VERSION: u32 = vk::API_VERSION_1_3;

/// A Vulkan instance with its loader and (optional) debug messenger.
///
/// Field order matters: `Drop` destroys the messenger and instance first, and then the fields
/// drop in declaration order, so `debug_state` (referenced by the messenger) and `entry` (the
/// loaded library) outlive every Vulkan object.
pub(crate) struct VulkanInstance {
    pub(crate) instance: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    validation: bool,
    surface_support: bool,
    debug_utils: bool,
    debug_state: Box<DebugState>,
    pub(crate) entry: ash::Entry,
}

impl std::fmt::Debug for VulkanInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanInstance")
            .field("instance", &self.instance.handle())
            .field("validation", &self.validation)
            .field("debug_messenger", &self.debug.is_some())
            .finish_non_exhaustive()
    }
}

impl VulkanInstance {
    /// Loads Vulkan and creates a 1.3 instance.
    pub(crate) fn new(app_name: &str, validation: Validation) -> Result<Self, RhiError> {
        // SAFETY: loading the Vulkan loader library has no preconditions beyond it being a
        // conforming loader. Failure is reported, not assumed.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| RhiError::RuntimeUnavailable {
            api: "Vulkan",
            details: e.to_string(),
        })?;

        // SAFETY: `entry` is a loaded loader. This call has no other preconditions.
        let version = unsafe { entry.try_enumerate_instance_version() }
            .map_err(|e| from_vk("vkEnumerateInstanceVersion", e))?
            .unwrap_or(vk::API_VERSION_1_0);
        if version < MIN_API_VERSION {
            return Err(RhiError::RuntimeTooOld {
                api: "Vulkan",
                found: format_version(version),
                required: "1.3".to_owned(),
            });
        }

        let layers = enumerate_complete(|| {
            // SAFETY: `entry` is a loaded loader.
            unsafe { entry.enumerate_instance_layer_properties() }
        })
        .map_err(|e| from_vk("vkEnumerateInstanceLayerProperties", e))?;
        // The layer's own version, logged below: validation behaviour (and its bugs) differs
        // between versions, so logs must say which one ran.
        let validation_layer_version = layers
            .iter()
            .find(|l| l.layer_name_as_c_str().is_ok_and(|n| n == VALIDATION_LAYER))
            .map(|l| l.spec_version);
        let layer_available = validation_layer_version.is_some();
        let env_override = validation_override(std::env::var(VALIDATION_ENV_VAR).ok().as_deref())?;
        let decision = resolve_validation(
            validation,
            env_override,
            layer_available,
            cfg!(debug_assertions),
        );
        if let Some(reason) = decision.warning {
            warn!("{reason}");
        }
        let validation = decision.enabled;
        let sync_validation = match parse_switch(
            SYNC_VALIDATION_ENV_VAR,
            std::env::var(SYNC_VALIDATION_ENV_VAR).ok().as_deref(),
        )? {
            Some(true) => {
                // Requested explicitly, so it must happen: silently running without it would
                // make "zero validation errors" meaningless.
                let unavailable = || RhiError::InvalidEnvironment {
                    variable: SYNC_VALIDATION_ENV_VAR,
                    value: "1".to_owned(),
                    expected: "`0`, or install the Vulkan validation layers: synchronization \
                               validation needs them and their VK_EXT_validation_features",
                };
                if !validation {
                    return Err(unavailable());
                }
                let layer_extensions = enumerate_complete(|| {
                    // SAFETY: `entry` is a loaded loader, and the layer name is a valid C string
                    // of an available layer.
                    unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER)) }
                })
                .map_err(|e| from_vk("vkEnumerateInstanceExtensionProperties", e))?;
                let has_features = layer_extensions.iter().any(|e| {
                    e.extension_name_as_c_str()
                        .is_ok_and(|n| n == ash::ext::validation_features::NAME)
                });
                if !has_features {
                    return Err(unavailable());
                }
                true
            }
            Some(false) | None => false,
        };

        let extensions = enumerate_complete(|| {
            // SAFETY: `entry` is a loaded loader. `None` lists the implementation's extensions.
            unsafe { entry.enumerate_instance_extension_properties(None) }
        })
        .map_err(|e| from_vk("vkEnumerateInstanceExtensionProperties", e))?;
        let available = |name: &CStr| {
            extensions
                .iter()
                .any(|e| e.extension_name_as_c_str().is_ok_and(|n| n == name))
        };
        let debug_utils_available = available(ash::ext::debug_utils::NAME);
        // Presentation: the generic surface extension plus every platform surface the loader
        // offers. One binary serves Win32, Wayland and X11 (ADR-0015). Missing ones just mean
        // that kind of window can't be presented to, which surface creation reports.
        let surface_extensions: Vec<&CStr> = [
            ash::khr::surface::NAME,
            ash::khr::win32_surface::NAME,
            ash::khr::wayland_surface::NAME,
            ash::khr::xlib_surface::NAME,
            ash::khr::xcb_surface::NAME,
        ]
        .into_iter()
        .filter(|name| available(name))
        .collect();
        let surface_support = surface_extensions.contains(&ash::khr::surface::NAME);

        let app_name = CString::new(app_name.replace('\0', "")).unwrap_or_default();
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(c"Oxreal")
            .engine_version(engine_version())
            .api_version(MIN_API_VERSION);
        let layer_names: Vec<*const std::ffi::c_char> = if validation {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            Vec::new()
        };
        let mut extension_names: Vec<*const std::ffi::c_char> = Vec::new();
        if debug_utils_available {
            extension_names.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        if surface_support {
            extension_names.extend(surface_extensions.iter().map(|name| name.as_ptr()));
        }
        if sync_validation {
            extension_names.push(ash::ext::validation_features::NAME.as_ptr());
        }

        let debug_state = Box::<DebugState>::default();
        let mut instance_messenger = messenger_create_info(&debug_state);
        let sync_features = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
        let mut validation_features =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&sync_features);
        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layer_names)
            .enabled_extension_names(&extension_names);
        if debug_utils_available {
            // Also captures messages from instance creation and destruction themselves.
            create_info = create_info.push_next(&mut instance_messenger);
        }
        if sync_validation {
            create_info = create_info.push_next(&mut validation_features);
        }

        // SAFETY: every pointer in `create_info` (application info, names, layer and extension
        // lists, the chained messenger and validation-features infos, and the messenger's user
        // data) refers to locals or to `debug_state`, all alive across this call. The layer and
        // extensions were checked to be available. VK_EXT_validation_features comes from the
        // enabled validation layer.
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|e| from_vk("vkCreateInstance", e))?;

        let mut this = Self {
            instance,
            debug: None,
            validation,
            surface_support,
            debug_utils: debug_utils_available,
            debug_state,
            entry,
        };

        if debug_utils_available {
            let loader = ash::ext::debug_utils::Instance::new(&this.entry, &this.instance);
            let info = messenger_create_info(&this.debug_state);
            // SAFETY: the instance was created with VK_EXT_debug_utils enabled. `info`'s user
            // data points at `this.debug_state`, which outlives the messenger (see `Drop`).
            let messenger = unsafe { loader.create_debug_utils_messenger(&info, None) }
                .map_err(|e| from_vk("vkCreateDebugUtilsMessengerEXT", e))?;
            this.debug = Some((loader, messenger));
        }

        info!(
            vulkan = %format_version(version),
            validation = this.validation,
            validation_layer = %validation_layer_version
                .filter(|_| this.validation)
                .map_or_else(|| "none".to_owned(), format_version),
            sync_validation,
            debug_utils = debug_utils_available,
            surfaces = surface_support,
            "Vulkan instance created"
        );
        Ok(this)
    }

    pub(crate) fn validation_enabled(&self) -> bool {
        self.validation
    }

    /// Whether `VK_KHR_surface` (and the platform surface extensions available) are enabled.
    pub(crate) fn surface_support(&self) -> bool {
        self.surface_support
    }

    /// Whether `VK_EXT_debug_utils` is enabled (object names and command labels).
    pub(crate) fn debug_utils(&self) -> bool {
        self.debug_utils
    }

    pub(crate) fn validation_error_count(&self) -> u64 {
        self.debug_state.error_count()
    }
}

impl Drop for VulkanInstance {
    fn drop(&mut self) {
        if let Some((loader, messenger)) = self.debug.take() {
            // SAFETY: the messenger was created from this instance and is destroyed exactly
            // once, before the instance.
            unsafe { loader.destroy_debug_utils_messenger(messenger, None) };
        }
        // SAFETY: every child object (device, messenger) is destroyed before this point:
        // `Shared` (the only owner of a `VulkanInstance` next to a device) declares its logical
        // device before its instance, so the device is destroyed first. Surfaces are destroyed
        // in `Shared::drop`, before either.
        unsafe { self.instance.destroy_instance(None) };
    }
}

/// Parses `OXREAL_VULKAN_VALIDATION`.
fn validation_override(value: Option<&str>) -> Result<Option<bool>, RhiError> {
    parse_switch(VALIDATION_ENV_VAR, value)
}

/// Parses an on/off environment switch: `1`, `true`, `on` or `0`, `false`, `off`.
fn parse_switch(variable: &'static str, value: Option<&str>) -> Result<Option<bool>, RhiError> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some("1" | "true" | "on") => Ok(Some(true)),
        Some("0" | "false" | "off") => Ok(Some(false)),
        Some(other) => Err(RhiError::InvalidEnvironment {
            variable,
            value: other.to_owned(),
            expected: "`1` or `0`",
        }),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ValidationDecision {
    enabled: bool,
    warning: Option<&'static str>,
}

/// Decides whether validation layers are used (ADR-0012 §5).
fn resolve_validation(
    requested: Validation,
    env_override: Option<bool>,
    layer_available: bool,
    debug_build: bool,
) -> ValidationDecision {
    let wanted = match (env_override, requested) {
        (Some(forced), _) => forced,
        (None, Validation::Enabled) => true,
        (None, Validation::Disabled) => false,
        (None, Validation::Automatic) => debug_build,
    };
    match (wanted, layer_available) {
        (true, true) => ValidationDecision {
            enabled: true,
            warning: None,
        },
        (true, false) => ValidationDecision {
            enabled: false,
            warning: Some(
                "Vulkan validation requested but VK_LAYER_KHRONOS_validation is not installed \
                 (install the Vulkan SDK or your distribution's validation-layers package); \
                 continuing without validation",
            ),
        },
        (false, _) => ValidationDecision {
            enabled: false,
            warning: None,
        },
    }
}

/// How many times a loader enumeration is attempted before `VK_INCOMPLETE` is reported. The
/// observed failure is transient (about 1 call sequence in 9 test runs fails once), so four
/// consecutive failures would mean something persistent, which deserves the error.
const ENUMERATION_ATTEMPTS: u32 = 4;

/// Runs a loader-assembled enumeration (layers, instance and device extensions, physical
/// devices) until it returns a complete list.
///
/// These lists are two calls: one for the count, one to fill. ash retries when the fill call
/// returns `VK_INCOMPLETE` (the array was too small because the list grew in between), but its
/// count call treats every non-`VK_SUCCESS` code as an error. The specification doesn't expect
/// `VK_INCOMPLETE` from a count-only call. Still, on Windows with AMD's driver installed it was
/// seen there, while other threads created and destroyed instances (about 1 run in 9 of the
/// parallel GPU tests); the loader, the driver and its implicit layers all take part in these
/// calls, so which one returns it isn't known. `VK_INCOMPLETE` is a success code, so the
/// enumeration starts over instead of failing.
///
/// The bound applies to `VK_INCOMPLETE` from the count call: after [`ENUMERATION_ATTEMPTS`] the
/// error is reported. ash's own retry of the fill call is not bounded by this.
pub(crate) fn enumerate_complete<T>(
    mut enumerate: impl FnMut() -> VkResult<Vec<T>>,
) -> VkResult<Vec<T>> {
    let mut attempt = 1;
    loop {
        match enumerate() {
            Err(vk::Result::INCOMPLETE) if attempt < ENUMERATION_ATTEMPTS => {
                debug!(
                    attempt,
                    "Vulkan loader enumeration was incomplete; asking again"
                );
                attempt += 1;
            }
            result => return result,
        }
    }
}

pub(crate) fn format_version(version: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(version),
        vk::api_version_minor(version),
        vk::api_version_patch(version)
    )
}

/// Oxreal's own version, reported to drivers and tools.
fn engine_version() -> u32 {
    let part = |s: &str| s.parse::<u32>().unwrap_or(0);
    vk::make_api_version(
        0,
        part(env!("CARGO_PKG_VERSION_MAJOR")),
        part(env!("CARGO_PKG_VERSION_MINOR")),
        part(env!("CARGO_PKG_VERSION_PATCH")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An enumeration that answers with `results` in turn, counting its calls.
    fn scripted(
        results: Vec<VkResult<Vec<u8>>>,
    ) -> (
        impl FnMut() -> VkResult<Vec<u8>>,
        std::rc::Rc<std::cell::Cell<u32>>,
    ) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = std::rc::Rc::clone(&calls);
        let mut results = results.into_iter();
        let enumerate = move || {
            counter.set(counter.get() + 1);
            results.next().expect("called more often than scripted")
        };
        (enumerate, calls)
    }

    #[test]
    fn an_incomplete_enumeration_is_asked_again() {
        let (enumerate, calls) = scripted(vec![Err(vk::Result::INCOMPLETE), Ok(vec![7])]);
        assert_eq!(enumerate_complete(enumerate), Ok(vec![7]));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn an_enumeration_that_never_completes_is_an_error() {
        let (enumerate, calls) = scripted(vec![
            Err(vk::Result::INCOMPLETE);
            ENUMERATION_ATTEMPTS as usize
        ]);
        assert_eq!(enumerate_complete(enumerate), Err(vk::Result::INCOMPLETE));
        assert_eq!(calls.get(), ENUMERATION_ATTEMPTS);
    }

    #[test]
    fn other_errors_are_not_retried() {
        let (enumerate, calls) = scripted(vec![Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY)]);
        assert_eq!(
            enumerate_complete(enumerate),
            Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY)
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn automatic_validation_follows_the_build_profile() {
        let on = resolve_validation(Validation::Automatic, None, true, true);
        assert!(on.enabled && on.warning.is_none());
        let off = resolve_validation(Validation::Automatic, None, true, false);
        assert!(!off.enabled);
    }

    #[test]
    fn environment_overrides_the_request() {
        assert!(!resolve_validation(Validation::Enabled, Some(false), true, true).enabled);
        assert!(resolve_validation(Validation::Disabled, Some(true), true, false).enabled);
    }

    #[test]
    fn missing_layer_warns_instead_of_failing() {
        let d = resolve_validation(Validation::Enabled, None, false, true);
        assert!(!d.enabled);
        assert!(
            d.warning
                .is_some_and(|w| w.contains("VK_LAYER_KHRONOS_validation"))
        );
        // Not wanted and not installed: nothing to warn about.
        assert!(
            resolve_validation(Validation::Disabled, None, false, true)
                .warning
                .is_none()
        );
    }

    #[test]
    fn parses_the_validation_override() {
        assert_eq!(validation_override(None).unwrap(), None);
        assert_eq!(validation_override(Some("")).unwrap(), None);
        assert_eq!(validation_override(Some("1")).unwrap(), Some(true));
        assert_eq!(validation_override(Some(" off ")).unwrap(), Some(false));
        let err = validation_override(Some("maybe")).unwrap_err();
        assert!(
            err.to_string().contains("OXREAL_VULKAN_VALIDATION"),
            "{err}"
        );
    }

    #[test]
    fn switches_name_their_variable_in_errors() {
        assert_eq!(
            parse_switch(SYNC_VALIDATION_ENV_VAR, Some("on")).unwrap(),
            Some(true)
        );
        let err = parse_switch(SYNC_VALIDATION_ENV_VAR, Some("yes")).unwrap_err();
        assert!(
            err.to_string().contains("OXREAL_VULKAN_SYNC_VALIDATION"),
            "{err}"
        );
    }

    #[test]
    fn versions_format_as_major_minor_patch() {
        assert_eq!(
            format_version(vk::make_api_version(0, 1, 3, 280)),
            "1.3.280"
        );
    }
}
