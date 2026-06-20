//! Deserialization for `Target.Describe` JSON.
//!
//! PLAN's named weakest assumption: the key set (version/newest/available/
//! installed/obsolete/incomplete/changelog/contents) is documented for systemd
//! 261 but its stability across 257->261 is unverified. To avoid failing on a
//! shape surprise, every documented scalar is an `Option` (missing key -> None),
//! the shape-uncertain `changelog`/`contents` stay as raw `serde_json::Value`,
//! and `#[serde(flatten)] extra` captures anything else. Deserialization of a
//! syntactically valid Describe payload therefore never errors on unexpected or
//! absent keys.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize)]
pub struct Describe {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub newest: Option<bool>,
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    pub installed: Option<bool>,
    #[serde(default)]
    pub obsolete: Option<bool>,
    #[serde(default)]
    pub incomplete: Option<bool>,
    /// Shape varies (string or array of strings depending on version); kept raw.
    #[serde(default)]
    pub changelog: Option<Value>,
    /// Array of per-resource objects; kept raw and rendered defensively.
    #[serde(default)]
    pub contents: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Deserialization for `Target.DescribeFeature` JSON. Documented keys (systemd
/// 261): name/description/enabled/documentationUrl/appstreamUrl/transfers. Same
/// defensive shape as `Describe`: every key optional, unknowns flattened.
#[derive(Debug, Default, Deserialize)]
pub struct Feature {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, rename = "documentationUrl")]
    pub documentation_url: Option<String>,
    #[serde(default, rename = "appstreamUrl")]
    pub appstream_url: Option<String>,
    #[serde(default)]
    pub transfers: Option<Value>,
}

impl Feature {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

impl Describe {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// A scalar `extra` key as a string, for defensive rendering of fields not
    /// promoted to a typed slot (e.g. size/timestamp keys that vary by version).
    pub fn extra_str(&self, key: &str) -> Option<String> {
        match self.extra.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// `extra` key as an integer, for `format_size`/`format_date` reuse.
    pub fn extra_i64(&self, key: &str) -> Option<i64> {
        self.extra.get(key)?.as_i64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_payload() {
        let json = r#"{
            "version": "2.3",
            "newest": true,
            "available": true,
            "installed": false,
            "obsolete": false,
            "incomplete": false,
            "changelog": "fixed stuff",
            "contents": [{"path": "/usr", "type": "regular"}]
        }"#;
        let d = Describe::parse(json).unwrap();
        assert_eq!(d.version.as_deref(), Some("2.3"));
        assert_eq!(d.newest, Some(true));
        assert_eq!(d.installed, Some(false));
        assert!(d.changelog.is_some());
        assert!(d.contents.is_some());
    }

    #[test]
    fn missing_keys_become_none() {
        let d = Describe::parse(r#"{"version": "1.0"}"#).unwrap();
        assert_eq!(d.version.as_deref(), Some("1.0"));
        assert_eq!(d.newest, None);
        assert_eq!(d.installed, None);
        assert!(d.changelog.is_none());
    }

    #[test]
    fn unknown_keys_captured_not_rejected() {
        let d = Describe::parse(r#"{"version": "1.0", "size": 1048576, "type": "url"}"#).unwrap();
        assert_eq!(d.extra_i64("size"), Some(1048576));
        assert_eq!(d.extra_str("type").as_deref(), Some("url"));
    }

    #[test]
    fn empty_object_is_all_none() {
        let d = Describe::parse("{}").unwrap();
        assert!(d.version.is_none());
        assert!(d.extra.is_empty());
    }

    #[test]
    fn feature_parses_documented_keys() {
        let json = r#"{
            "name": "experimental",
            "description": "bleeding edge",
            "enabled": true,
            "documentationUrl": "https://example/doc",
            "appstreamUrl": "https://example/as.xml",
            "transfers": ["a.transfer", "b.transfer"]
        }"#;
        let f = Feature::parse(json).unwrap();
        assert_eq!(f.name.as_deref(), Some("experimental"));
        assert_eq!(f.enabled, Some(true));
        assert_eq!(f.documentation_url.as_deref(), Some("https://example/doc"));
        assert_eq!(f.appstream_url.as_deref(), Some("https://example/as.xml"));
        assert!(f.transfers.is_some());
    }

    #[test]
    fn feature_missing_and_unknown_keys() {
        let f = Feature::parse(r#"{"name": "x", "futureKey": 1}"#).unwrap();
        assert_eq!(f.name.as_deref(), Some("x"));
        assert_eq!(f.enabled, None);
        assert!(f.description.is_none());
    }
}
