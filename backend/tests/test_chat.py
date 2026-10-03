import json
from types import SimpleNamespace

import pytest

from pennychest.ai import providers
from pennychest.ai.providers import ChatReply, ProviderError, ToolCall
from pennychest.chat import service
from tests.test_ai import api  # noqa: F401  (fixture)

TOOLS = [
    {
        "name": "list_categories",
        "description": "List categories.",
        "input_schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }
]

HISTORY = [
    {"role": "user", "content": "How much on food?"},
    {
        "role": "assistant",
        "content": "Let me check.",
        "raw": None,
        "raw_provider": None,
        "tool_calls": [
            {"id": "t1", "name": "list_categories", "arguments": {}},
            {"id": "t2", "name": "spending_summary", "arguments": {"start_date": "2026-09-01"}},
        ],
    },
    {
        "role": "tool",
        "tool_call_id": "t1",
        "name": "list_categories",
        "content": "{}",
        "is_error": False,
    },
    {
        "role": "tool",
        "tool_call_id": "t2",
        "name": "spending_summary",
        "content": '{"error": "x"}',
        "is_error": True,
    },
]


def openai_stream(*chunks) -> str:
    return "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"


def delta(**fields) -> dict:
    return {"choices": [{"index": 0, "delta": fields, "finish_reason": None}]}


def finish(reason: str) -> dict:
    return {"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}


def parse_sse(text: str) -> list[tuple[str, dict]]:
    events = []
    for block in text.strip().split("\n\n"):
        lines = dict(line.split(": ", 1) for line in block.splitlines())
        events.append((lines["event"], json.loads(lines["data"])))
    return events


# Provider request formats


def test_anthropic_history_groups_tool_results_and_replays_raw():
    messages = providers.AnthropicProvider._messages(HISTORY)
    assert messages[1] == {
        "role": "assistant",
        "content": [
            {"type": "text", "text": "Let me check."},
            {"type": "tool_use", "id": "t1", "name": "list_categories", "input": {}},
            {
                "type": "tool_use",
                "id": "t2",
                "name": "spending_summary",
                "input": {"start_date": "2026-09-01"},
            },
        ],
    }
    assert messages[2] == {
        "role": "user",
        "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "{}", "is_error": False},
            {
                "type": "tool_result",
                "tool_use_id": "t2",
                "content": '{"error": "x"}',
                "is_error": True,
            },
        ],
    }

    raw = {
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "s"},
            {"type": "text", "text": "Hi"},
        ],
    }
    replayed = providers.AnthropicProvider._messages(
        [{"role": "assistant", "content": "Hi", "raw": raw, "raw_provider": "anthropic"}]
    )
    assert replayed == [raw]
    # Another provider's raw message is converted instead of replayed.
    converted = providers.AnthropicProvider._messages(
        [{"role": "assistant", "content": "Hi", "raw": {"x": 1}, "raw_provider": "openai"}]
    )
    assert converted == [{"role": "assistant", "content": [{"type": "text", "text": "Hi"}]}]


def _block(**fields):
    return SimpleNamespace(**fields, model_dump=lambda mode=None, exclude_none=False: dict(fields))


def test_anthropic_streams_text_then_the_reply(monkeypatch):
    calls = []

    class FakeStream:
        text_stream = ["Check", "ing."]

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def get_final_message(self):
            return SimpleNamespace(
                stop_reason="tool_use",
                content=[
                    _block(type="thinking", thinking="", signature="sig"),
                    _block(type="text", text="Checking."),
                    _block(
                        type="tool_use", id="toolu_1", name="list_categories", input={"kind": "all"}
                    ),
                ],
                model_dump=lambda mode=None: {"id": "msg_1"},
            )

    def stream(**request):
        calls.append(request)
        return FakeStream()

    monkeypatch.setattr(
        providers,
        "anthropic_client",
        lambda key: SimpleNamespace(messages=SimpleNamespace(stream=stream)),
    )
    items = list(
        providers.PROVIDERS["anthropic"].stream_chat_turn(
            {"api_key": "k"},
            "claude-opus-5",
            "system text",
            [{"role": "user", "content": "hi"}],
            TOOLS,
        )
    )
    assert items[:2] == ["Check", "ing."]
    reply = items[-1]
    request = calls[0]
    assert request["system"] == "system text"
    assert request["tools"] == [
        {
            "name": "list_categories",
            "description": "List categories.",
            "input_schema": TOOLS[0]["input_schema"],
        }
    ]
    assert request["cache_control"] == {"type": "ephemeral"}
    assert "tool_choice" not in request
    assert reply.text == "Checking."
    assert reply.tool_calls == [ToolCall("toolu_1", "list_categories", {"kind": "all"})]
    assert reply.raw["content"][0] == {"type": "thinking", "thinking": "", "signature": "sig"}

    # On the last round the tools stay listed but Claude can't call them
    list(
        providers.PROVIDERS["anthropic"].stream_chat_turn(
            {"api_key": "k"}, "claude-opus-5", "s", [], TOOLS, answer_only=True
        )
    )
    assert calls[1]["tool_choice"] == {"type": "none"} and calls[1]["tools"]


def test_openai_streams_and_reassembles_tool_calls(api):  # noqa: F811
    url = "https://api.openai.com/v1/chat/completions"
    api.on(
        "POST",
        url,
        (
            200,
            openai_stream(
                delta(role="assistant", content="Let me "),
                delta(content="check."),
                delta(
                    tool_calls=[
                        {
                            "index": 0,
                            "id": "call_9",
                            "type": "function",
                            "function": {"name": "list_categories", "arguments": '{"ki'},
                        }
                    ]
                ),
                delta(tool_calls=[{"index": 0, "function": {"arguments": 'nd": "expense"}'}}]),
                finish("tool_calls"),
            ),
        ),
    )
    openai = providers.PROVIDERS["openai"]
    items = list(openai.stream_chat_turn({"api_key": "sk"}, "gpt-5", "sys", HISTORY, TOOLS))
    assert items[:2] == ["Let me ", "check."]
    reply = items[-1]

    sent = api.body()
    assert sent["stream"] is True
    assert sent["messages"][0] == {"role": "system", "content": "sys"}
    assert sent["messages"][2]["tool_calls"][1]["function"] == {
        "name": "spending_summary",
        "arguments": '{"start_date": "2026-09-01"}',
    }
    assert sent["messages"][3] == {"role": "tool", "tool_call_id": "t1", "content": "{}"}
    assert sent["tools"][0] == {
        "type": "function",
        "function": {
            "name": "list_categories",
            "description": "List categories.",
            "parameters": TOOLS[0]["input_schema"],
        },
    }
    assert reply.text == "Let me check."
    assert reply.tool_calls == [ToolCall("call_9", "list_categories", {"kind": "expense"})]
    assert reply.raw == {
        "role": "assistant",
        "content": "Let me check.",
        "tool_calls": [
            {
                "id": "call_9",
                "type": "function",
                "function": {"name": "list_categories", "arguments": '{"kind": "expense"}'},
            },
        ],
    }


def test_openai_stream_errors_are_readable(api):  # noqa: F811
    api.on(
        "POST",
        "https://api.openai.com/v1/chat/completions",
        (401, {"error": {"message": "bad key"}}),
    )
    with pytest.raises(ProviderError, match="rejected the credentials"):
        providers.PROVIDERS["openai"].chat_turn({"api_key": "sk"}, "gpt-5", "sys", [], TOOLS)


def test_gemini_thought_signatures_survive_replay(api):  # noqa: F811
    url = "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
    api.on(
        "POST",
        url,
        (
            200,
            openai_stream(
                delta(
                    role="assistant",
                    tool_calls=[
                        {
                            "index": 0,
                            "id": "c1",
                            "type": "function",
                            "function": {"name": "list_categories", "arguments": "{}"},
                            "extra_content": {"google": {"thought_signature": "SIG"}},
                        }
                    ],
                ),
                finish("tool_calls"),
            ),
        ),
        (200, openai_stream(delta(content="Done"), finish("stop"))),
    )
    gemini = providers.PROVIDERS["google"]
    first = gemini.chat_turn(
        {"api_key": "g"}, "gemini-3", "sys", [{"role": "user", "content": "hi"}], TOOLS
    )
    history = [
        {"role": "user", "content": "hi"},
        {
            "role": "assistant",
            "content": "",
            "raw": first.raw,
            "raw_provider": "google",
            "tool_calls": [{"id": "c1", "name": "list_categories", "arguments": {}}],
        },
        {"role": "tool", "tool_call_id": "c1", "name": "list_categories", "content": "{}"},
    ]
    assert gemini.chat_turn({"api_key": "g"}, "gemini-3", "sys", history, TOOLS).text == "Done"
    assert api.body()["messages"][2]["tool_calls"] == [
        {
            "id": "c1",
            "type": "function",
            "function": {"name": "list_categories", "arguments": "{}"},
            "extra_content": {"google": {"thought_signature": "SIG"}},
        }
    ]


def test_ollama_streams_and_numbers_tool_calls(api):  # noqa: F811
    lines = [
        {"message": {"role": "assistant", "content": "Look"}, "done": False},
        {"message": {"role": "assistant", "content": "ing"}, "done": False},
        {
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {"function": {"name": "list_categories", "arguments": {"kind": "income"}}}
                ],
            },
            "done": False,
        },
        {"message": {"role": "assistant", "content": ""}, "done": True},
    ]
    api.on(
        "POST",
        "http://gpu:11434/api/chat",
        (200, "\n".join(json.dumps(line) for line in lines) + "\n"),
    )
    items = list(
        providers.PROVIDERS["ollama"].stream_chat_turn(
            {"url": "http://gpu:11434"}, "qwen3:4b", "sys", HISTORY, TOOLS
        )
    )
    assert items[:2] == ["Look", "ing"]
    reply = items[-1]
    sent = api.body()
    assert sent["stream"] is True
    assert sent["messages"][3] == {"role": "tool", "tool_name": "list_categories", "content": "{}"}
    assert sent["messages"][2]["tool_calls"][0] == {
        "function": {"name": "list_categories", "arguments": {}},
    }
    assert reply.text == "Looking"
    assert reply.tool_calls == [ToolCall("call_0", "list_categories", {"kind": "income"})]


def test_jev_cannot_chat():
    with pytest.raises(ProviderError, match="can't be used for chat"):
        providers.PROVIDERS["typesafe"].chat_turn({"api_key": "k"}, "jev-latest", "s", [], TOOLS)


# The chat loop, with a scripted model


class ScriptedModel:
    id = "scripted"
    label = "Scripted"

    def __init__(self, *replies):
        self.replies = list(replies)
        self.calls = []

    def stream_chat_turn(self, cfg, model, system, history, tools, answer_only=False):
        self.calls.append(
            {
                "system": system,
                "history": json.loads(json.dumps(history)),
                "tools": [t["name"] for t in tools],
                "answer_only": answer_only,
            }
        )
        step = self.replies.pop(0) if self.replies else self.replies_default()
        if isinstance(step, Exception):
            raise step
        text, calls = step
        half = len(text) // 2
        yield from (part for part in (text[:half], text[half:]) if part)
        yield ChatReply(
            text=text,
            tool_calls=[ToolCall(f"id{i}", name, args) for i, (name, args) in enumerate(calls)],
            raw={"marker": text},
            request={"model": model},
            response={"ok": True},
        )

    def replies_default(self):
        return ("", [("list_categories", {})])


@pytest.fixture
def model(monkeypatch):
    def use(*replies):
        scripted = ScriptedModel(*replies)
        monkeypatch.setattr(service, "resolve_task", lambda db, task: (scripted, {}, "m1"))
        return scripted

    return use


def send(client, message, conversation_id=None):
    body = {"message": message}
    if conversation_id is not None:
        body["conversation_id"] = conversation_id
    response = client.post("/api/chat/messages", json=body)
    assert response.status_code == 200, response.text
    assert response.headers["content-type"].startswith("text/event-stream")
    return parse_sse(response.text)


def test_chat_runs_read_actions_and_answers(client, model):
    client.post(
        "/api/accounts", json={"name": "Food", "full_path": "Expenses:Food", "type": "expense"}
    )
    scripted = model(
        ("Let me look.", [("list_categories", {"kind": "expense"})]),
        ("You have one spending category: Food.", []),
    )
    events = send(client, "What categories do I have?")
    assert [e for e, _ in events] == [
        "conversation",
        "text",
        "text",
        "tool",
        "tool_result",
        "text",
        "text",
        "message",
        "done",
    ]
    conversation = events[0][1]
    assert conversation["title"] == "What categories do I have?"
    assert "".join(e[1]["delta"] for e in events[1:3]) == "Let me look."
    assert events[3][1] == {"name": "list_categories", "arguments": {"kind": "expense"}}
    assert events[4][1] == {"name": "list_categories", "ok": True}
    assert "".join(e[1]["delta"] for e in events[5:7]) == "You have one spending category: Food."
    assert events[7][1] == {"content": "You have one spending category: Food."}

    offered = set(scripted.calls[0]["tools"])
    assert "spending_summary" in offered and "create_category" not in offered
    assert "Today is" in scripted.calls[0]["system"]
    second_history = scripted.calls[1]["history"]
    assert [m["role"] for m in second_history] == ["user", "assistant", "tool"]
    assert json.loads(second_history[2]["content"])["categories"][0]["path"] == "Expenses:Food"
    assert second_history[1]["raw"] == {"marker": "Let me look."}
    assert second_history[1]["raw_provider"] == "scripted"

    turns = client.get(f"/api/chat/conversations/{conversation['id']}").json()["turns"]
    assert turns == [
        {"role": "user", "content": "What categories do I have?"},
        {
            "role": "assistant",
            "content": "You have one spending category: Food.",
            "tools": [{"name": "list_categories", "ok": True}],
            "changes": [],
        },
    ]
    logs = client.get("/api/ai/logs").json()["logs"]
    assert [(log["operation"], log["provider"]) for log in logs] == [
        ("chat", "scripted:m1"),
        ("chat", "scripted:m1"),
    ]


def test_follow_up_messages_continue_the_conversation(client, model):
    scripted = model(("First answer.", []), ("Second answer.", []))
    conversation_id = send(client, "Hello")[0][1]["id"]
    events = send(client, "And another thing", conversation_id)
    assert events[0][1]["id"] == conversation_id
    assert [m["content"] for m in scripted.calls[1]["history"]] == [
        "Hello",
        "First answer.",
        "And another thing",
    ]
    assert len(client.get("/api/chat/conversations").json()) == 1


def test_write_actions_are_refused_in_read_only_chat(client, model):
    client.post(
        "/api/accounts", json={"name": "Expenses", "full_path": "Expenses", "type": "expense"}
    )
    scripted = model(
        ("", [("create_category", {"name": "Gym", "parent": "Expenses"}), ("no_such_tool", {})]),
        ("I can't make changes yet.", []),
    )
    events = send(client, "Create a Gym category")
    assert ("tool_result", {"name": "create_category", "ok": False}) in events
    assert ("tool_result", {"name": "no_such_tool", "ok": False}) in events
    results = scripted.calls[1]["history"][2:]
    assert "'organise' permission" in results[0]["content"] and results[0]["is_error"]
    assert "no action called" in results[1]["content"]
    categories = client.post("/api/actions/list_categories", json={}).json()["categories"]
    assert "Expenses:Gym" not in [c["path"] for c in categories]


def test_last_round_must_answer_with_what_it_has(client, model):
    lookups = [("", [("list_categories", {})])] * (service.MAX_ROUNDS - 1)
    scripted = model(*lookups, ("Cancel the gym: £22.99 a month.", []))
    events = send(client, "What's my quickest win for saving money?")
    assert len(scripted.calls) == service.MAX_ROUNDS
    assert not scripted.calls[-2]["answer_only"]
    assert service.LAST_ROUND not in scripted.calls[-2]["system"]
    assert scripted.calls[-1]["answer_only"]
    assert scripted.calls[-1]["system"].endswith(service.LAST_ROUND)
    assert events[-2] == ("message", {"content": "Cancel the gym: £22.99 a month."})


def test_tool_calls_on_the_last_round_are_dropped_but_the_answer_kept(client, model):
    # As from Ollama, which can't be stopped from calling tools
    scripted = model(*[("", [("list_categories", {})])] * (service.MAX_ROUNDS - 1))
    scripted.replies_default = lambda: ("Here's what I found so far.", [("list_categories", {})])
    events = send(client, "Loop forever")
    assert events[-2] == ("message", {"content": "Here's what I found so far."})
    conversation_id = events[0][1]["id"]
    turns = client.get(f"/api/chat/conversations/{conversation_id}").json()["turns"]
    assert len(turns[-1]["tools"]) == service.MAX_ROUNDS - 1

    # The stored history has no unanswered tool calls, so the conversation can carry on
    model(("Sure.", []))
    events = send(client, "Thanks", conversation_id)
    assert events[-2] == ("message", {"content": "Sure."})


def test_chat_gives_up_if_the_last_round_writes_nothing(client, model):
    scripted = model()  # always asks for another tool, with no text
    events = send(client, "Loop forever")
    assert len(scripted.calls) == service.MAX_ROUNDS
    assert events[-2][0] == "message" and "reasonable number of steps" in events[-2][1]["content"]
    turns = client.get(f"/api/chat/conversations/{events[0][1]['id']}").json()["turns"]
    assert turns[-1]["role"] == "assistant" and len(turns[-1]["tools"]) == service.MAX_ROUNDS - 1


def test_provider_errors_are_reported_and_logged(client, model):
    model(ProviderError("Anthropic rejected the API key."))
    events = send(client, "Hi")
    assert events[-2] == ("error", {"detail": "Anthropic rejected the API key."})
    assert events[-1][0] == "done"
    assert (
        client.get("/api/ai/logs").json()["logs"][0]["error"] == "Anthropic rejected the API key."
    )


def test_chat_needs_a_configured_model(client):
    events = send(client, "Hi")
    assert events[1][0] == "error" and "Settings > AI" in events[1][1]["detail"]


def test_conversations_list_titles_and_delete(client, model):
    model(("ok", []), ("ok", []))
    long_message = "Please summarise " + "all of my spending " * 10
    first = send(client, long_message)[0][1]
    assert len(first["title"]) <= service.TITLE_LENGTH and first["title"].endswith("…")
    second = send(client, "Budgets?")[0][1]
    assert [c["id"] for c in client.get("/api/chat/conversations").json()] == [
        second["id"],
        first["id"],
    ]

    assert client.delete(f"/api/chat/conversations/{first['id']}").status_code == 204
    assert client.get(f"/api/chat/conversations/{first['id']}").status_code == 404
    assert (
        client.post(
            "/api/chat/messages", json={"message": "x", "conversation_id": first["id"]}
        ).status_code
        == 404
    )
    assert client.post("/api/chat/messages", json={"message": ""}).status_code == 422
    assert client.post("/api/chat/messages", json={"message": "   "}).status_code == 400


def test_chat_requires_session(anon_client):
    assert anon_client.get("/api/chat/conversations").status_code == 401
    assert anon_client.post("/api/chat/messages", json={"message": "hi"}).status_code == 401
