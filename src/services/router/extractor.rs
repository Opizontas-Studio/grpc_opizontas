use super::error::RouterError;
use std::path::Path;

// 增强的服务名解析，支持多种格式
pub fn extract_service_name(path: &str) -> Result<String, RouterError> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(RouterError::InvalidPath(
            "Path must start with '/'".to_string(),
        ));
    }

    // 使用 Path 来处理路径结构
    let path_obj = Path::new(path);
    let mut components = path_obj.components().skip(1); // 跳过根路径 "/"

    // 获取第一个组件作为服务路径
    let service_component = components
        .next()
        .ok_or_else(|| RouterError::InvalidPath("Path must have service component".to_string()))?;

    // 检查是否还有方法组件
    if components.next().is_none() {
        return Err(RouterError::InvalidPath(
            "Path must have at least service and method parts".to_string(),
        ));
    }

    let service_path = service_component
        .as_os_str()
        .to_str()
        .ok_or_else(|| RouterError::InvalidPath("Invalid UTF-8 in service path".to_string()))?;

    // 支持多种服务名格式
    let service_name = if service_path.contains('.') {
        // 标准格式: "package.ServiceName" -> "ServiceName"
        service_path.split('.').next_back().unwrap_or(service_path)
    } else {
        // 简单格式: "ServiceName"
        service_path
    };

    if service_name.is_empty() {
        return Err(RouterError::InvalidPath("Empty service name".to_string()));
    }

    Ok(service_name.to_string())
}
