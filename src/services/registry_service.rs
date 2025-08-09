use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

use crate::registry::{
    registry_service_server::RegistryService, RegisterRequest, RegisterResponse,
};

// 定义服务注册表的共享类型
pub type ServiceRegistry = Arc<Mutex<HashMap<String, String>>>;

// 定义的服务实现
#[derive(Debug, Default)]
pub struct MyRegistryService {
    pub registry: ServiceRegistry,
}

// 为结构体实现 gRPC 服务 trait
#[tonic::async_trait]
impl RegistryService for MyRegistryService {
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();
        let mut registry = self.registry.lock().unwrap();

        for service_name in req.services {
            println!(
                "Registering service '{}' at address '{}'",
                &service_name, &req.address
            );
            registry.insert(service_name, req.address.clone());
        }

        let reply = RegisterResponse {
            success: true,
            message: "Registration successful".into(),
        };

        Ok(Response::new(reply))
    }
}