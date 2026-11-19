//! Creates a Vulkan device and reports what was chosen. A quick check of a machine's setup.
//!
//! ```text
//! cargo run -p oxreal-rhi-vulkan --example device_info
//! OXREAL_GPU=1 cargo run -p oxreal-rhi-vulkan --example device_info   # pick another GPU
//! ```
//!
//! Exits with an error if no usable GPU is found, or if validation reported errors.

use oxreal_diagnostics::info;
use oxreal_rhi::{Device as _, DeviceDesc, Validation};

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(run)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal device_info".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;

    let a = device.adapter_info();
    info!(
        name = %a.name,
        kind = ?a.adapter_type,
        vendor_id = format_args!("{:#06x}", a.vendor_id),
        device_id = format_args!("{:#06x}", a.device_id),
        driver = %a.driver_name,
        driver_info = %a.driver_info,
        vulkan = %a.api_version,
        validation = device.validation_enabled(),
        "selected GPU"
    );

    device.wait_idle()?;
    let errors = device.validation_error_count();
    drop(device);
    if errors > 0 {
        return Err(
            format!("Vulkan validation reported {errors} error(s); see the log above").into(),
        );
    }
    info!("device created and destroyed cleanly");
    Ok(())
}
