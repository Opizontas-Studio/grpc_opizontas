use crate::registry::registry_service_server::RegistryServiceServer;
use crate::services::registry_service::MyRegistryService;
use crate::services::router_service::DynamicRouter;
use tonic::transport::Server;

pub async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    // 创建服务实例
    let registry_service = MyRegistryService::default();
    let registry = registry_service.registry.clone();

    println!("Gateway server listening on {} with registry service", addr);
    println!("Dynamic routing will be integrated via custom router middleware");

    // 创建动态路由器
    let _router = DynamicRouter::new(registry.clone());
    
    // 启动服务器
    Server::builder()
        .add_service(RegistryServiceServer::new(registry_service))
        .serve(addr)
        .await?;

    Ok(())
}