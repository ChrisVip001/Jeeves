//! Wire types for app-server v2 and the memory center.

pub mod memory_center;
pub mod v2;

#[cfg(test)]
#[path = "v2_tests.rs"]
mod v2_tests;
