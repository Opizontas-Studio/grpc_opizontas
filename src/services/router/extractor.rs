use super::error::RouterError;

// 增强的服务名解析，支持多种格式
pub fn extract_service_name(path: &str) -> Result<String, RouterError> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(RouterError::InvalidPath(
            "Path must start with '/'".to_string(),
        ));
    }

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.len() < 2 {
        return Err(RouterError::InvalidPath(
            "Path must have at least service and method parts".to_string(),
        ));
    }

    let service_path = parts[0];

    // 支持多种服务名格式
    let service_name = if service_path.contains('.') {
        // 标准格式: "package.ServiceName" -> "ServiceName"
        service_path.split('.').last().unwrap_or(service_path)
    } else {
        // 简单格式: "ServiceName"
        service_path
    };

    if service_name.is_empty() {
        return Err(RouterError::InvalidPath("Empty service name".to_string()));
    }

    Ok(service_name.to_string())
}