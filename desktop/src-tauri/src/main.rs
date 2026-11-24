#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "cloud-device")]
mod cloud_device;
#[cfg(not(feature = "cloud-device"))]
mod development;
#[cfg(not(feature = "cloud-device"))]
mod notice;
#[cfg(not(feature = "cloud-device"))]
mod summon;
#[cfg(not(feature = "cloud-device"))]
mod window_state;

fn main() {
    #[cfg(feature = "cloud-device")]
    cloud_device::main();
    #[cfg(not(feature = "cloud-device"))]
    development::main();
}
