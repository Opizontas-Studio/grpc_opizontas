use super::error::GatewayClientError;

/// 静态方法：序列化消息（优化内存使用）
pub fn serialize_message_static<T: prost::Message>(
    message: &T,
) -> Result<Vec<u8>, GatewayClientError> {
    // 估算消息大小以减少重新分配
    let estimated_size = message.encoded_len();
    let mut buf = bytes::BytesMut::with_capacity(estimated_size);

    message
        .encode(&mut buf)
        .map_err(|e| GatewayClientError::Serialization(e.to_string()))?;

    // 转换为Vec<u8>以保持API兼容性
    Ok(buf.to_vec())
}

/// 静态方法：反序列化响应
pub fn deserialize_response_static<R: prost::Message + Default>(
    response_bytes: Vec<u8>,
) -> Result<R, GatewayClientError> {
    R::decode(&response_bytes[..]).map_err(|e| GatewayClientError::Serialization(e.to_string()))
}
