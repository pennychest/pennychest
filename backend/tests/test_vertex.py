import json

from pennychest.ai import providers
from pennychest.ai.config import set_provider_values
from tests.conftest import use_model
from tests.test_ai import _expense_account, _imported_transactions, api  # noqa: F401

BASE = "https://aiplatform.googleapis.com/v1/publishers/google/models"
VERTEX = providers.PROVIDERS["vertex"]


def _sse(*chunks):
    return "".join(f"data: {json.dumps(c)}\r\n\r\n" for c in chunks)


def test_refresh_checks_the_key_and_lists_models(client, api):  # noqa: F811
    client.put("/api/ai/providers/vertex", json={"values": {"api_key": "AQ.good"}})
    api.on("POST", f"{BASE}/gemini-2.5-flash:countTokens", (200, {"totalTokens": 1}))
    response = client.post("/api/ai/providers/vertex/models")
    assert response.status_code == 200
    ids = [m["id"] for m in response.json()["models"]]
    assert "gemini-2.5-flash" in ids
    assert api.requests[-1].headers["x-goog-api-key"] == "AQ.good"
    assert "key=" not in str(api.requests[-1].url)


def test_a_rejected_key_is_shown_until_replaced(client, api, db_session):  # noqa: F811
    client.put("/api/ai/providers/google", json={"values": {"api_key": "AQ.vertex-key"}})
    use_model(db_session, "chat", "google", "gemini-2.5-flash")
    api.on(
        "GET",
        "https://generativelanguage.googleapis.com/v1beta/openai/models",
        (401, {"error": {"message": "API keys are not supported by this API."}}),
    )
    assert client.post("/api/ai/providers/google/models").status_code == 400

    config = client.get("/api/ai/config").json()
    google = next(p for p in config["providers"] if p["id"] == "google")
    assert google["configured"] is True
    assert "rejected the credentials" in google["problem"]
    assert "API keys are not supported" in google["problem"]
    assert config["tasks"]["chat"]["ready"] is False
    assert "rejected the credentials" in config["tasks"]["chat"]["problem"]

    # Saving a new key clears it until that key is tried
    client.put("/api/ai/providers/google", json={"values": {"api_key": "AIza-new"}})
    google = next(
        p for p in client.get("/api/ai/config").json()["providers"] if p["id"] == "google"
    )
    assert google["problem"] is None


def test_an_unreachable_provider_is_not_marked_broken(client, api):  # noqa: F811
    client.put("/api/ai/providers/vertex", json={"values": {"api_key": "AQ.good"}})
    api.on("POST", f"{BASE}/gemini-2.5-flash:countTokens", (503, {"error": "overloaded"}))
    assert client.post("/api/ai/providers/vertex/models").status_code == 400
    vertex = next(
        p for p in client.get("/api/ai/config").json()["providers"] if p["id"] == "vertex"
    )
    assert vertex["problem"] is None


def test_categorise_with_vertex_structured_output(client, api, db_session):  # noqa: F811
    (tesco,), _ = _imported_transactions(client, "TESCO STORES")
    set_provider_values(db_session, "vertex", {"api_key": "AQ.good"})
    use_model(db_session, "categorise", "vertex", "gemini-2.5-flash")
    answer = {
        "categorisations": [{"transaction_id": tesco, "account_full_path": "Expenses:Groceries"}]
    }
    api.on(
        "POST",
        f"{BASE}/gemini-2.5-flash:generateContent",
        (
            200,
            {
                "candidates": [
                    {
                        "content": {"role": "model", "parts": [{"text": json.dumps(answer)}]},
                        "finishReason": "STOP",
                    }
                ],
                "usageMetadata": {"promptTokenCount": 120, "candidatesTokenCount": 30},
            },
        ),
    )
    assert client.post("/api/ai/categorise/all").json() == {"updated": 1}
    assert _expense_account(client, tesco) == "Expenses:Groceries"
    sent = api.body()
    assert sent["generationConfig"]["responseMimeType"] == "application/json"
    assert "Expenses:Groceries" in json.dumps(sent["generationConfig"]["responseJsonSchema"])
    assert "TESCO STORES" in sent["contents"][0]["parts"][0]["text"]


def test_vertex_chat_streams_and_calls_tools(api):  # noqa: F811
    signature = "sig-123"
    api.on(
        "POST",
        f"{BASE}/gemini-2.5-flash:streamGenerateContent?alt=sse",
        (
            200,
            _sse(
                {"candidates": [{"content": {"role": "model", "parts": [{"text": "Let me "}]}}]},
                {
                    "candidates": [
                        {
                            "content": {
                                "role": "model",
                                "parts": [
                                    {"text": "check."},
                                    {
                                        "functionCall": {"name": "list_categories", "args": {}},
                                        "thoughtSignature": signature,
                                    },
                                ],
                            },
                            "finishReason": "STOP",
                        }
                    ]
                },
            ),
        ),
    )
    tools = [{"name": "list_categories", "description": "List", "input_schema": {"type": "object"}}]
    items = list(
        VERTEX.stream_chat_turn(
            {"api_key": "AQ.good"},
            "gemini-2.5-flash",
            "Be brief.",
            [{"role": "user", "content": "What categories do I have?"}],
            tools,
        )
    )
    assert items[:2] == ["Let me ", "check."]
    reply = items[-1]
    assert reply.text == "Let me check."
    assert [(c.name, c.arguments) for c in reply.tool_calls] == [("list_categories", {})]
    sent = api.body()
    assert sent["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"] == {"type": "object"}
    assert sent["systemInstruction"]["parts"][0]["text"] == "Be brief."

    # The next round replays the model's turn verbatim and returns the results together
    history = [
        {"role": "user", "content": "What categories do I have?"},
        {
            "role": "assistant",
            "content": reply.text,
            "tool_calls": [{"id": "call_0", "name": "list_categories", "arguments": {}}],
            "raw": reply.raw,
            "raw_provider": "vertex",
        },
        {
            "role": "tool",
            "tool_call_id": "call_0",
            "name": "list_categories",
            "content": json.dumps({"categories": ["Food"]}),
            "is_error": False,
        },
    ]
    contents = VERTEX._contents(history)
    [call] = [part for part in contents[1]["parts"] if "functionCall" in part]
    assert call["thoughtSignature"] == signature
    assert contents[2] == {
        "role": "user",
        "parts": [
            {"functionResponse": {"name": "list_categories", "response": {"categories": ["Food"]}}}
        ],
    }


def test_vertex_refusals_and_cut_offs(api):  # noqa: F811
    import pytest

    api.on(
        "POST",
        f"{BASE}/gemini-2.5-flash:generateContent",
        (200, {"promptFeedback": {"blockReason": "SAFETY"}}),
        (
            200,
            {"candidates": [{"content": {"parts": [{"text": "{"}]}, "finishReason": "MAX_TOKENS"}]},
        ),
    )
    with pytest.raises(providers.ProviderError, match="declined"):
        VERTEX.complete_json({"api_key": "k"}, "gemini-2.5-flash", "s", "p", {}, "n")
    with pytest.raises(providers.ProviderError, match="cut off"):
        VERTEX.complete_json({"api_key": "k"}, "gemini-2.5-flash", "s", "p", {}, "n")


def test_googles_invalid_key_400_counts_as_rejected(client, api):  # noqa: F811
    client.put("/api/ai/providers/google", json={"values": {"api_key": "AIza-bad"}})
    api.on(
        "GET",
        "https://generativelanguage.googleapis.com/v1beta/openai/models",
        (400, {"error": {"code": 400, "message": "Please pass a valid API key"}}),
    )
    assert client.post("/api/ai/providers/google/models").status_code == 400
    google = next(
        p for p in client.get("/api/ai/config").json()["providers"] if p["id"] == "google"
    )
    assert "valid API key" in google["problem"]
