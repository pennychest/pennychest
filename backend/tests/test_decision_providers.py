import json

import httpx

from pennychest.ai import providers


def test_openai_decisions_answers_jev_style_questions(monkeypatch):
    sent = {}

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url == "https://api.openai.com/v1/decisions"
        assert request.headers["authorization"] == "Bearer sk-1"
        sent.update(json.loads(request.content))
        return httpx.Response(200, json={"answers": [
            {"type": "choice", "name": "c", "choice": "b", "probabilities": [], "confidence": 0.9},
            {"type": "predicate", "name": "n", "probability": 0.7},
            {"type": "score", "name": "s", "score": 2.4, "probabilities": [], "confidence": 0.8},
            {"type": "refusal", "name": "r"},
        ]})

    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(handler))
    call = providers.PROVIDERS["openai_decisions"].choose(
        {"api_key": "sk-1"},
        "gpt-6-luna",
        {"t1": "TESCO STORES"},
        {
            "c": {"type": "choice", "instructions": "Which?", "criteria": {"a": None, "b": "Bee"}},
            "n": {
                "type": "noul",
                "instructions": "Is it?",
                "criteria": {"true": "Yes", "false": "No"},
            },
            "s": {"type": "score", "instructions": "How much?", "criteria": ["Low", "High"]},
            "r": {"type": "noul", "instructions": "Well?"},
        },
    )

    assert call.data == {
        "c": {"choice": "b", "confidence": 0.9},
        "n": {"noul": 0.7},
        "s": {"score": 2.4, "confidence": 0.8},
        "r": {},
    }
    assert sent["model"] == "gpt-6-luna"
    assert json.loads(sent["input"]) == {"t1": "TESCO STORES"}
    questions = {q["name"]: q for q in sent["questions"]}
    assert questions["c"]["choices"] == [
        {"value": "a", "description": "a"},
        {"value": "b", "description": "Bee"},
    ]
    assert questions["n"] == {
        "type": "predicate",
        "name": "n",
        "instructions": "Is it?\nTrue: Yes\nFalse: No",
    }
    assert questions["s"]["levels"] == [
        {"label": "0", "description": "Low"},
        {"label": "1", "description": "High"},
    ]
    assert questions["r"] == {"type": "predicate", "name": "r", "instructions": "Well?"}


def test_openai_decisions_lists_its_model_once_the_key_works(monkeypatch):
    monkeypatch.setattr(
        providers,
        "http_transport",
        httpx.MockTransport(lambda request: httpx.Response(200, json={"data": []})),
    )
    models = providers.PROVIDERS["openai_decisions"].list_models({"api_key": "sk-1"})
    assert models == [{"id": "gpt-6-luna", "label": "GPT-6 Luna"}]


def test_both_decision_providers_are_decision_models():
    kinds = {p.id: p.kind for p in providers.PROVIDERS.values()}
    assert kinds["typesafe"] == kinds["openai_decisions"] == "decision"
    assert {k for k, v in kinds.items() if v == "llm"} == {
        "anthropic", "openai", "google", "vertex", "ollama"
    }
