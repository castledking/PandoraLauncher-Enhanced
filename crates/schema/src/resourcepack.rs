use std::sync::Arc;

use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct PackMcmeta {
    pub pack: PackMcmetaPack,
}

#[derive(Deserialize, Debug)]
pub struct PackMcmetaPack {
    /// Description can be a string or a text component (object/array) in newer pack formats
    #[serde(deserialize_with = "deserialize_pack_description")]
    pub description: Arc<str>,
}

fn deserialize_pack_description<'de, D>(deserializer: D) -> Result<Arc<str>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_json::Value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::String(s) => s.into(),
        serde_json::Value::Object(obj) => {
            // Text component: try "text" or "translate" for display
            obj.get("text")
                .or(obj.get("translate"))
                .and_then(|v| v.as_str())
                .map(Arc::from)
                .unwrap_or_else(|| Arc::from(""))
        }
        serde_json::Value::Array(arr) => {
            // Array of text components - take first with "text" or "translate"
            arr.iter()
                .find_map(|v| v.as_object())
                .and_then(|obj| obj.get("text").or(obj.get("translate")))
                .and_then(|v| v.as_str())
                .map(Arc::from)
                .unwrap_or_else(|| Arc::from(""))
        }
        serde_json::Value::Null | _ => Arc::from(""),
    })
}
