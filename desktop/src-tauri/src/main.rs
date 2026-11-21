#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "cloud-device")]
mod cloud_device;
#[cfg(not(feature = "cloud-device"))]
mod development;

fn main() {
    #[cfg(feature = "cloud-device")]
    cloud_device::main();
    #[cfg(not(feature = "cloud-device"))]
    development::main();
}
