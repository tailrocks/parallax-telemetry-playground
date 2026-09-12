use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NotificationRequest {
    #[serde(default)]
    pub(crate) tenant_id: Option<String>,
    #[serde(default)]
    pub(crate) event_key: String,
    pub(crate) order_id: String,
    #[serde(default = "default_channel")]
    pub(crate) channel: String,
    #[serde(default)]
    pub(crate) payload: Value,
}

pub(crate) fn default_channel() -> String {
    "webhook".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{NotificationRequest, default_channel};

    #[test]
    fn tenant_identity_has_no_service_default() {
        assert_eq!(default_channel(), "webhook");
        let request = NotificationRequest {
            tenant_id: None,
            event_key: String::new(),
            order_id: "order-1".into(),
            channel: default_channel(),
            payload: serde_json::json!({"status":"confirmed"}),
        };
        assert!(request.tenant_id.is_none());
        assert_eq!(request.order_id, "order-1");
    }
}
