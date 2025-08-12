use super::error::GatewayClientError;

/// 静态方法：序列化消息
pub fn serialize_message_static<T: prost::Message>(message: &T) -> Result<Vec<u8>, GatewayClientError> {
    let mut buf = Vec::new();
    message.encode(&mut buf)
        .map_err(|e| GatewayClientError::Serialization(e.to_string()))?;
    Ok(buf)
}

/// 静态方法：反序列化响应
pub fn deserialize_response_static<R: prost::Message + Default>(response_bytes: Vec<u8>) -> Result<R, GatewayClientError> {
    R::decode(&response_bytes[..])
        .map_err(|e| GatewayClientError::Serialization(e.to_string()))
}