//! Device creation against a real Vulkan implementation.
//!
//! These tests need a Vulkan 1.3 device and the validation layers. They're ignored by default.
//! Run them with `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use oxreal_rhi::{Device as _, DeviceDesc, GpuSelection, RhiError, Validation};

fn validated_desc() -> DeviceDesc {
    // Validation messages go to the log; with a subscriber, a failing test shows them.
    let _ = oxreal_diagnostics::init_application_logging();
    DeviceDesc {
        app_name: "oxreal-rhi-vulkan tests".to_owned(),
        validation: Validation::Enabled,
        gpu: GpuSelection::Automatic,
    }
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers; run with --ignored"]
fn creates_and_destroys_a_device_without_validation_errors() {
    let device = oxreal_rhi_vulkan::create_device(&validated_desc()).expect("device creation");

    let info = device.adapter_info();
    assert!(!info.name.is_empty());
    assert!(
        (info.api_version.major, info.api_version.minor) >= (1, 3),
        "{}",
        info.api_version
    );
    assert!(
        device.validation_enabled(),
        "validation layers must be installed for this test (Vulkan SDK or vulkan-validationlayers)"
    );

    device.wait_idle().expect("wait idle");
    assert_eq!(
        device.validation_error_count(),
        0,
        "validation reported errors (see log)"
    );
    drop(device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers; run with --ignored"]
fn an_unknown_gpu_name_is_a_clear_error() {
    let desc = DeviceDesc {
        gpu: GpuSelection::NameContains("no-such-gpu-oxreal-test".to_owned()),
        ..validated_desc()
    };
    match oxreal_rhi_vulkan::create_device(&desc) {
        Err(RhiError::AdapterNotFound { available, .. }) => {
            assert!(!available.is_empty(), "the error lists usable adapters");
        }
        other => panic!("expected AdapterNotFound, got {other:?}"),
    }
}
