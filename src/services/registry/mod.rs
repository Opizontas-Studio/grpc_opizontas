//! Registry service module
//! 
//! This module contains the service registry implementation split into logical components:
//! - `types`: Data structures and type definitions
//! - `service`: Core service logic and methods
//! - `grpc_impl`: gRPC trait implementation

pub mod types;
pub mod service;
pub mod grpc_impl;

// Re-export public types for easier access
pub use types::{ServiceInfo, ServiceHealthStatus, ServiceRegistry};
pub use service::MyRegistryService;