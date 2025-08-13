use crate::config::Config;
use crate::registry::registry_service_server::RegistryServiceServer;
use crate::services::registry::MyRegistryService;
use crate::services::router::DynamicRouter;
use tonic::transport::Server;

pub async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    // 加载配置
    let config = Config::load()?;
    tracing::info!("Security configuration loaded successfully");

    // 创建服务实例
    let registry_service = MyRegistryService::new(config.clone());
    let registry = registry_service.registry.clone();
    let reverse_manager = registry_service.reverse_connection_manager.clone();

    // 创建动态路由器
    let router = DynamicRouter::new(registry.clone(), config.clone(), reverse_manager);

    tracing::info!("Gateway server listening on {} with registry service", addr);
    tracing::info!("Dynamic routing enabled for all gRPC requests");

    // 启动服务器，将动态路由器作为主要的服务处理器
    // 注册服务请求会被动态路由器识别并转发到注册服务
    Server::builder()
        .add_service(tower::ServiceBuilder::new().service(router))
        .add_service(RegistryServiceServer::new(registry_service))
        .serve(addr)
        .await?;

    Ok(())
}
