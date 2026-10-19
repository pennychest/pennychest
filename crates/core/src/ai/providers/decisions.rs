//! Decision model providers: TypeSafe's Jev, whose question format the tasks use, and OpenAI's
//! Decisions API, which answers the same kinds of question under other names.

use serde_json::{Map, Value, json};

use super::{Call, DecisionModel, ProviderConfig, ProviderError, send, setting};
use crate::ai::config::ModelInfo;
use crate::ai::http::{Request, Transport};

pub const TYPESAFE: &str = "TypeSafe AI (Jev)";
pub const OPENAI: &str = "OpenAI Decisions";

fn authorised(request: Request, cfg: &ProviderConfig) -> Request {
    request.header(
        "Authorization",
        format!("Bearer {}", setting(cfg, "api_key")),
    )
}

pub struct TypeSafe;

const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1";

impl TypeSafe {
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        let body = send(
            http,
            TYPESAFE,
            authorised(Request::get(format!("{TYPESAFE_URL}/models")), cfg),
        )
        .await?;
        Ok(body["models"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m["name"].as_str().filter(|n| !n.is_empty()))
            .map(|name| ModelInfo {
                id: name.to_owned(),
                label: name.to_owned(),
            })
            .collect())
    }
}

impl DecisionModel for TypeSafe {
    async fn choose(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        model: &str,
        state: &Value,
        questions: &Map<String, Value>,
    ) -> Result<Call, ProviderError> {
        let request = json!({"state": state, "model": model, "questions": questions});
        let body = send(
            http,
            TYPESAFE,
            authorised(
                Request::post(format!("{TYPESAFE_URL}/systemone"), request.clone()),
                cfg,
            ),
        )
        .await?;
        let Some(answers) = body["answers"].as_object().cloned() else {
            return Err(ProviderError::new(format!(
                "{TYPESAFE} returned an unexpected response."
            )));
        };
        Ok(Call {
            data: answers,
            request,
            response: body,
        })
    }
}

pub struct OpenAiDecisions;

const OPENAI_URL: &str = "https://api.openai.com/v1";
/// OpenAI's model list doesn't say which models take decisions; in the beta there's one.
const OPENAI_MODELS: [(&str, &str); 1] = [("gpt-6-luna", "GPT-6 Luna")];

impl OpenAiDecisions {
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        // Listing OpenAI's models checks the key works.
        send(
            http,
            OPENAI,
            authorised(Request::get(format!("{OPENAI_URL}/models")), cfg),
        )
        .await?;
        Ok(OPENAI_MODELS
            .iter()
            .map(|(id, label)| ModelInfo {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
            })
            .collect())
    }

    /// A Jev question as OpenAI asks it: a noul is a predicate, and a score's criteria are
    /// levels.
    fn question(name: &str, question: &Value) -> Result<Value, ProviderError> {
        let instructions = question["instructions"].as_str().unwrap_or_default();
        let criteria = &question["criteria"];
        Ok(match question["type"].as_str() {
            Some("choice") => {
                let choices: Vec<Value> = criteria
                    .as_object()
                    .map(|c| {
                        c.iter()
                            .map(|(value, description)| {
                                let description = description.as_str().filter(|d| !d.is_empty());
                                json!({"value": value, "description": description.unwrap_or(value)})
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({"type": "choice", "name": name, "instructions": instructions, "choices": choices})
            }
            Some("noul") => {
                let mut instructions = instructions.to_owned();
                if super::truthy(criteria) {
                    instructions.push_str(&format!(
                        "\nTrue: {}\nFalse: {}",
                        super::text_of(&criteria["true"]),
                        super::text_of(&criteria["false"])
                    ));
                }
                json!({"type": "predicate", "name": name, "instructions": instructions})
            }
            Some("score") => {
                let levels: Vec<Value> = criteria
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(level, description)| {
                        json!({"label": level.to_string(), "description": description})
                    })
                    .collect();
                json!({"type": "score", "name": name, "instructions": instructions, "levels": levels})
            }
            other => {
                return Err(ProviderError::new(format!(
                    "Unknown question type {}",
                    other.unwrap_or("(none)")
                )));
            }
        })
    }

    fn answer(answer: &Value) -> Value {
        match answer["type"].as_str() {
            Some("choice") => {
                json!({"choice": answer["choice"], "confidence": answer["confidence"]})
            }
            Some("predicate") => json!({"noul": answer["probability"]}),
            Some("score") => json!({"score": answer["score"], "confidence": answer["confidence"]}),
            // A refusal
            _ => json!({}),
        }
    }
}

impl DecisionModel for OpenAiDecisions {
    async fn choose(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        model: &str,
        state: &Value,
        questions: &Map<String, Value>,
    ) -> Result<Call, ProviderError> {
        let questions: Vec<Value> = questions
            .iter()
            .map(|(name, q)| Self::question(name, q))
            .collect::<Result<_, _>>()?;
        let request = json!({
            "model": model,
            "input": state.to_string(),
            "questions": questions,
        });
        let body = send(
            http,
            OPENAI,
            authorised(
                Request::post(format!("{OPENAI_URL}/decisions"), request.clone()),
                cfg,
            ),
        )
        .await?;
        let Some(answers) = body["answers"].as_array() else {
            return Err(ProviderError::new(format!(
                "{OPENAI} returned an unexpected response."
            )));
        };
        let data = answers
            .iter()
            .filter(|a| a.is_object())
            .map(|a| (super::text_of(&a["name"]), Self::answer(a)))
            .collect();
        Ok(Call {
            data,
            request,
            response: body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::http::FakeApi;

    #[tokio::test]
    async fn openai_decisions_answers_jev_style_questions() {
        let api = FakeApi::new();
        api.on(
            "POST",
            "https://api.openai.com/v1/decisions",
            200,
            json!({"answers": [
                {"type": "choice", "name": "c", "choice": "b", "probabilities": [], "confidence": 0.9},
                {"type": "predicate", "name": "n", "probability": 0.7},
                {"type": "score", "name": "s", "score": 2.4, "probabilities": [], "confidence": 0.8},
                {"type": "refusal", "name": "r"},
            ]}),
        );
        let cfg = ProviderConfig::from([("api_key".into(), "sk-1".into())]);
        let questions = json!({
            "c": {"type": "choice", "instructions": "Which?", "criteria": {"a": null, "b": "Bee"}},
            "n": {"type": "noul", "instructions": "Is it?", "criteria": {"true": "Yes", "false": "No"}},
            "s": {"type": "score", "instructions": "How much?", "criteria": ["Low", "High"]},
            "r": {"type": "noul", "instructions": "Well?"},
        });
        let call = OpenAiDecisions
            .choose(
                api.as_ref(),
                &cfg,
                "gpt-6-luna",
                &json!({"t1": "TESCO STORES"}),
                questions.as_object().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            Value::Object(call.data),
            json!({
                "c": {"choice": "b", "confidence": 0.9},
                "n": {"noul": 0.7},
                "s": {"score": 2.4, "confidence": 0.8},
                "r": {},
            })
        );
        let request = &api.requests()[0];
        assert!(
            request
                .headers
                .contains(&("Authorization".into(), "Bearer sk-1".into()))
        );
        let sent = api.last_body();
        assert_eq!(sent["model"], "gpt-6-luna");
        let input: Value = serde_json::from_str(sent["input"].as_str().unwrap()).unwrap();
        assert_eq!(input, json!({"t1": "TESCO STORES"}));
        let by_name = |name: &str| {
            sent["questions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|q| q["name"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(
            by_name("c")["choices"],
            json!([{"value": "a", "description": "a"}, {"value": "b", "description": "Bee"}])
        );
        assert_eq!(
            by_name("n"),
            json!({"type": "predicate", "name": "n", "instructions": "Is it?\nTrue: Yes\nFalse: No"})
        );
        assert_eq!(
            by_name("s")["levels"],
            json!([{"label": "0", "description": "Low"}, {"label": "1", "description": "High"}])
        );
        assert_eq!(
            by_name("r"),
            json!({"type": "predicate", "name": "r", "instructions": "Well?"})
        );
    }

    #[tokio::test]
    async fn openai_decisions_lists_its_model_once_the_key_works() {
        let api = FakeApi::new();
        api.on(
            "GET",
            "https://api.openai.com/v1/models",
            200,
            json!({"data": []}),
        );
        let cfg = ProviderConfig::from([("api_key".into(), "sk-1".into())]);
        let models = OpenAiDecisions
            .list_models(api.as_ref(), &cfg)
            .await
            .unwrap();
        assert_eq!(
            models,
            [ModelInfo {
                id: "gpt-6-luna".into(),
                label: "GPT-6 Luna".into()
            }]
        );
    }
}
