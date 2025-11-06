//! Registry service module
//!
//! This module contains the service registry implementation split into logical components:
//! - `types`: Data structures and type definitions
//! - `service`: Core service logic and methods
//! - `grpc_impl`: gRPC trait implementation

pub mod grpc_impl;
pub mod service;
pub mod types;

// Re-export public types for easier access
pub use service::MyRegistryService;
pub use types::{ServiceHealthStatus, ServiceInfo, ServiceRegistry};
