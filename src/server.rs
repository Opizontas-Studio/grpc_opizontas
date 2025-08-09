use crate::registry::registry_service_server::RegistryServiceServer;
use crate::services::registry_service::{MyRegistryService, ServiceRegistry};
use tonic::transport::Server;

pub async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    // 初始化服务注册表
    let registry = ServiceRegistry::default();

    // 创建服务实例
    let registry_service = MyRegistryService {
        registry: registry.clone(),
    };

    println!("Gateway server listening on {}", addr);

    // 暂时只启动注册服务，动态路由功能待完善
    Server::builder()
        .add_service(RegistryServiceServer::new(registry_service))
        .serve(addr)
        .await?;

    Ok(())
}