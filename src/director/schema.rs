//! The JSON shapes the AI fills in for a place and a passage, plus their JSON schemas.
//!
//! Every string carries `maxLength` (twice the limit the prompt states, to bound runaway output)
//! and every array carries `maxItems`. The limits the engine actually enforces live in
//! `director::validate`.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::{Deserialize, Serialize};

use crate::pack::model::{ExitKind, HostOs, Stance, VisionKind};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaceGen {
    #[schemars(length(max = 80))]
    pub title: String,
    #[schemars(length(max = 48))]
    pub bbs_name: String,
    #[schemars(length(max = 120))]
    pub tagline: String,
    #[schemars(length(max = 640))]
    pub level_card: String,
    #[schemars(length(max = 32))]
    pub sysop: String,
    pub host: HostGen,
    #[schemars(length(max = 6))]
    pub posts: Vec<PostGen>,
    #[schemars(length(max = 4))]
    pub text_files: Vec<FileGen>,
    #[schemars(length(max = 4))]
    pub host_files: Vec<FileGen>,
    #[schemars(length(max = 28), inner(length(max = 24)))]
    pub words: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HostGen {
    #[schemars(length(max = 80))]
    pub name: String,
    pub os: HostOs,
    #[schemars(length(max = 320))]
    pub banner: String,
    #[schemars(length(max = 200))]
    pub objective: String,
    #[schemars(length(max = 24))]
    pub target_file: String,
    #[schemars(length(max = 16), inner(length(max = 48)))]
    pub node_names: Vec<String>,
    #[schemars(length(max = 6), inner(length(max = 40)))]
    pub ice_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PostGen {
    #[schemars(length(max = 32))]
    pub from: String,
    #[schemars(length(max = 80))]
    pub subject: String,
    #[schemars(length(max = 800))]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileGen {
    #[schemars(length(max = 24))]
    pub name: String,
    #[schemars(length(max = 80))]
    pub description: String,
    #[schemars(length(max = 800))]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PassageGen {
    #[schemars(length(max = 1400))]
    pub beat: String,
    pub choice: ChoiceGen,
    #[schemars(length(max = 6))]
    pub exits: Vec<ExitGen>,
    pub vision: VisionKind,
    #[schemars(length(max = 6), inner(length(max = 96)))]
    pub captions: Vec<String>,
    #[schemars(length(max = 400))]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChoiceGen {
    #[schemars(length(max = 320))]
    pub prompt: String,
    #[schemars(length(max = 6))]
    pub options: Vec<OptionGen>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OptionGen {
    #[schemars(length(max = 140))]
    pub text: String,
    pub stance: Stance,
    #[schemars(length(max = 4), inner(length(max = 64)))]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExitGen {
    pub kind: ExitKind,
    #[schemars(length(max = 120))]
    pub text: String,
}

fn schema_of<T: JsonSchema>() -> serde_json::Value {
    let generator = SchemaSettings::draft07()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// JSON schema (draft 07, sub-schemas inlined) for `PlaceGen`.
pub fn place_schema() -> serde_json::Value {
    schema_of::<PlaceGen>()
}

/// JSON schema (draft 07, sub-schemas inlined) for `PassageGen`.
pub fn passage_schema() -> serde_json::Value {
    schema_of::<PassageGen>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(m) => {
                out.push(v);
                m.values().for_each(|c| walk(c, out));
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }

    /// Every string-typed schema node has maxLength and every array node has maxItems.
    fn assert_bounded(schema: &Value) {
        let mut nodes = Vec::new();
        walk(schema, &mut nodes);
        let mut strings = 0;
        let mut arrays = 0;
        for n in nodes {
            match n.get("type").and_then(Value::as_str) {
                Some("string") if n.get("enum").is_none() => {
                    strings += 1;
                    assert!(n.get("maxLength").is_some(), "unbounded string: {n}");
                }
                Some("array") => {
                    arrays += 1;
                    assert!(n.get("maxItems").is_some(), "unbounded array: {n}");
                }
                _ => {}
            }
        }
        assert!(strings > 5 && arrays > 2);
    }

    #[test]
    fn schemas_bound_every_string_and_array() {
        assert_bounded(&place_schema());
        assert_bounded(&passage_schema());
    }

    #[test]
    fn place_schema_limits_are_twice_the_prompt_limits() {
        let place = place_schema();
        let props = &place["properties"];
        assert_eq!(props["posts"]["maxItems"], 6);
        assert_eq!(props["text_files"]["maxItems"], 4);
        assert_eq!(props["host_files"]["maxItems"], 4);
        assert_eq!(
            props["posts"]["items"]["properties"]["body"]["maxLength"],
            800
        );
        assert_eq!(
            props["posts"]["items"]["properties"]["subject"]["maxLength"],
            80
        );
        for list in ["text_files", "host_files"] {
            let item = &props[list]["items"]["properties"];
            assert_eq!(item["body"]["maxLength"], 800, "{list}");
            assert_eq!(item["description"]["maxLength"], 80, "{list}");
        }
    }

    #[test]
    fn schemas_are_inlined_draft07() {
        for schema in [place_schema(), passage_schema()] {
            let text = schema.to_string();
            assert!(!text.contains("\"$ref\""), "{text}");
            assert!(text.contains("draft-07"));
        }
        let place = place_schema();
        assert_eq!(
            place["properties"]["host"]["properties"]["os"]["enum"],
            serde_json::json!(["unix", "vax", "mainframe", "dos"])
        );
    }
}
