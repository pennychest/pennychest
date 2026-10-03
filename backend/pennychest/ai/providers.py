"""AI providers the app can use, what each one needs to connect, and which tasks it can do.

LLM providers implement `complete_json`, which returns JSON matching a schema. TypeSafe's
Jev is a decision model rather than an LLM: it answers typed Choice questions via `choose`,
so it can do every task except writing rules and chat.
"""

import json
import re
from collections.abc import Iterator
from dataclasses import dataclass

import anthropic
import httpx
from sqlalchemy.orm import Session

from pennychest.ai.config import TASKS, get_provider_problem, get_task, provider_value


class ProviderError(ValueError):
    """A provider problem whose message is safe and useful to show the user."""


class CredentialsError(ProviderError):
    """The provider rejected the saved key, as opposed to being busy or unreachable."""


@dataclass(frozen=True)
class Field:
    key: str
    label: str
    secret: bool = False
    required: bool = True
    default: str | None = None
    placeholder: str = ""


@dataclass
class Call:
    data: dict
    request: dict
    response: dict


@dataclass
class ToolCall:
    id: str
    name: str
    arguments: dict


@dataclass
class ChatReply:
    """One model turn: its text, any tools it wants run, and the message exactly as the
    provider returned it. `raw` is replayed verbatim when the conversation continues with the
    same provider, which keeps provider-specific data such as Claude's thinking blocks or
    Gemini's thought signatures intact."""

    text: str
    tool_calls: list[ToolCall]
    raw: dict
    request: dict
    response: dict


# Conversation history passed to chat_turn is a list of provider-neutral dicts:
#   {"role": "user", "content": str}
#   {"role": "assistant", "content": str, "tool_calls": [ToolCall-like dicts],
#    "raw": dict | None, "raw_provider": str | None}
#   {"role": "tool", "tool_call_id": str, "name": str, "content": str, "is_error": bool}
# Tools are {"name", "description", "input_schema"} dicts from the actions layer.


def _parse_arguments(label: str, value) -> dict:
    if isinstance(value, dict):
        return value
    try:
        parsed = json.loads(value or "{}")
    except ValueError:
        raise ProviderError(f"{label} asked to run a tool with arguments that weren't valid JSON.")
    return parsed if isinstance(parsed, dict) else {}


# Tests replace this with an httpx.MockTransport.
http_transport: httpx.BaseTransport | None = None


def _error_detail(response: httpx.Response) -> str:
    try:
        body = response.json()
    except ValueError:
        return response.text[:200]
    if isinstance(body, dict):
        error = body.get("error")
        if isinstance(error, dict) and error.get("message"):
            return str(error["message"])[:200]
        if isinstance(error, str):
            return error[:200]
        if body.get("detail"):
            return str(body["detail"])[:200]
    return json.dumps(body)[:200]


def _send(
    label: str,
    method: str,
    url: str,
    *,
    headers: dict | None = None,
    body: dict | None = None,
    timeout: float = 120.0,
) -> dict:
    try:
        with httpx.Client(transport=http_transport, timeout=timeout) as client:
            response = client.request(method, url, headers=headers, json=body)
    except httpx.TimeoutException:
        raise ProviderError(f"{label} took too long to respond.")
    except httpx.TransportError:
        raise ProviderError(f"Couldn't connect to {label} at {url}.")
    _raise_for_status(label, response)
    try:
        return response.json()
    except ValueError:
        raise ProviderError(f"{label} returned a response that wasn't JSON.")


def _raise_for_status(label: str, response: httpx.Response) -> None:
    status = response.status_code
    # Google answers an invalid key with 400 ("Please pass a valid API key") rather than 401
    invalid_key = status == 400 and "api key" in _error_detail(response).lower()
    if status in (401, 403) or invalid_key:
        raise CredentialsError(
            f"{label} rejected the credentials ({_error_detail(response)}). "
            "Check them in Settings > AI."
        )
    if status == 404:
        raise ProviderError(f"{label} couldn't find that model: {_error_detail(response)}")
    if status in (429, 529):
        raise ProviderError(f"{label} is busy or rate limiting requests. Try again shortly.")
    if status >= 400:
        raise ProviderError(f"{label} returned an error ({status}): {_error_detail(response)}")


def _stream_lines(
    label: str, url: str, *, headers: dict | None = None, body: dict, timeout: float = 300.0
) -> Iterator[str]:
    """POST and yield the response body line by line as it arrives."""
    try:
        with httpx.Client(transport=http_transport, timeout=timeout) as client:
            with client.stream("POST", url, headers=headers, json=body) as response:
                if response.status_code >= 400:
                    response.read()
                    _raise_for_status(label, response)
                yield from response.iter_lines()
    except httpx.TimeoutException:
        raise ProviderError(f"{label} took too long to respond.")
    except httpx.TransportError:
        raise ProviderError(f"Couldn't connect to {label} at {url}.")


def _parse_json(label: str, text: str | None) -> dict:
    try:
        data = json.loads(text or "")
    except ValueError:
        raise ProviderError(f"{label} returned output that wasn't valid JSON: {(text or '')[:200]}")
    if not isinstance(data, dict):
        raise ProviderError(f"{label} returned unexpected output: {str(data)[:200]}")
    return data


class Provider:
    id: str
    label: str
    fields: tuple[Field, ...]
    tasks: frozenset[str]

    def list_models(self, cfg: dict) -> list[dict]:
        raise NotImplementedError

    def complete_json(
        self, cfg: dict, model: str, system: str, prompt: str, schema: dict, name: str
    ) -> Call:
        raise ProviderError(f"{self.label} can't do this task.")

    def stream_chat_turn(
        self,
        cfg: dict,
        model: str,
        system: str,
        history: list[dict],
        tools: list[dict],
        answer_only: bool = False,
    ) -> Iterator[str | ChatReply]:
        """Yield the reply's text as it's generated, then the complete ChatReply last.
        With answer_only the tools stay listed (providers reject a history of tool calls
        without them) but the model is stopped from calling any, where the provider allows."""
        raise ProviderError(f"{self.label} can't be used for chat.")

    def chat_turn(
        self, cfg: dict, model: str, system: str, history: list[dict], tools: list[dict]
    ) -> ChatReply:
        reply = None
        for item in self.stream_chat_turn(cfg, model, system, history, tools):
            if isinstance(item, ChatReply):
                reply = item
        if reply is None:
            raise ProviderError(f"{self.label} ended the response early.")
        return reply


def anthropic_client(api_key: str) -> anthropic.Anthropic:
    return anthropic.Anthropic(api_key=api_key)


class AnthropicProvider(Provider):
    id = "anthropic"
    label = "Anthropic (Claude)"
    fields = (Field("api_key", "API key", secret=True, placeholder="sk-ant-..."),)
    tasks = frozenset(TASKS)

    @staticmethod
    def _error(e: anthropic.APIError) -> ProviderError:
        if isinstance(e, anthropic.AuthenticationError):
            return CredentialsError("Anthropic rejected the API key. Check it in Settings > AI.")
        if isinstance(e, anthropic.PermissionDeniedError):
            return CredentialsError("That Anthropic API key isn't allowed to use this model.")
        if isinstance(e, anthropic.NotFoundError):
            return ProviderError("Anthropic couldn't find that model. Refresh the model list.")
        if isinstance(e, anthropic.RateLimitError):
            return ProviderError("Anthropic is rate limiting requests. Try again shortly.")
        if isinstance(e, anthropic.BadRequestError):
            return ProviderError(f"Anthropic rejected the request: {e.message}")
        if isinstance(e, anthropic.APIStatusError):
            return ProviderError(
                f"Anthropic returned an error ({e.status_code}). Try again shortly."
            )
        return ProviderError("Couldn't connect to Anthropic.")

    def _call(self, fn):
        try:
            return fn()
        except anthropic.APIError as e:
            raise self._error(e)

    def list_models(self, cfg: dict) -> list[dict]:
        client = anthropic_client(cfg["api_key"])
        models = self._call(lambda: list(client.models.list()))
        result = []
        for m in models:
            capabilities = getattr(m, "capabilities", None)
            # Categorisation and rule suggestions rely on structured outputs.
            if isinstance(capabilities, dict) and not capabilities.get(
                "structured_outputs", {}
            ).get("supported", True):
                continue
            result.append({"id": m.id, "label": m.display_name or m.id})
        return result

    def complete_json(self, cfg, model, system, prompt, schema, name) -> Call:
        client = anthropic_client(cfg["api_key"])
        request = {
            "model": model,
            "max_tokens": 16000,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
            "output_config": {"format": {"type": "json_schema", "schema": schema}},
        }
        response = self._call(lambda: client.messages.create(**request))
        if response.stop_reason == "refusal":
            raise ProviderError("Claude declined this request.")
        if response.stop_reason == "max_tokens":
            raise ProviderError("Claude's answer was cut off. Try categorising fewer transactions.")
        text = next((b.text for b in response.content if b.type == "text"), None)
        return Call(_parse_json(self.label, text), request, response.model_dump(mode="json"))

    @staticmethod
    def _messages(history: list[dict]) -> list[dict]:
        messages: list[dict] = []
        for m in history:
            if m["role"] == "user":
                messages.append({"role": "user", "content": m["content"]})
            elif m["role"] == "assistant":
                if m.get("raw_provider") == "anthropic" and m.get("raw"):
                    messages.append(m["raw"])
                    continue
                blocks = [{"type": "text", "text": m["content"]}] if m.get("content") else []
                blocks += [
                    {"type": "tool_use", "id": c["id"], "name": c["name"], "input": c["arguments"]}
                    for c in m.get("tool_calls") or []
                ]
                if blocks:
                    messages.append({"role": "assistant", "content": blocks})
            else:
                block = {
                    "type": "tool_result",
                    "tool_use_id": m["tool_call_id"],
                    "content": m["content"],
                    "is_error": bool(m.get("is_error")),
                }
                # All results for one assistant turn go back in a single user message.
                last = messages[-1] if messages else None
                if (
                    last
                    and last["role"] == "user"
                    and isinstance(last["content"], list)
                    and last["content"][-1].get("type") == "tool_result"
                ):
                    last["content"].append(block)
                else:
                    messages.append({"role": "user", "content": [block]})
        return messages

    def stream_chat_turn(self, cfg, model, system, history, tools, answer_only=False):
        client = anthropic_client(cfg["api_key"])
        request = {
            "model": model,
            "max_tokens": 64000,
            "system": system,
            "messages": self._messages(history),
            "tools": [
                {
                    "name": t["name"],
                    "description": t["description"],
                    "input_schema": t["input_schema"],
                }
                for t in tools
            ],
            # Caches the tools, system prompt and history so each round only pays for new input.
            "cache_control": {"type": "ephemeral"},
        }
        if answer_only:
            request["tool_choice"] = {"type": "none"}
        try:
            with client.messages.stream(**request) as stream:
                yield from stream.text_stream
                response = stream.get_final_message()
        except anthropic.APIError as e:
            raise self._error(e)
        if response.stop_reason == "refusal":
            raise ProviderError("Claude declined this request.")
        if response.stop_reason == "max_tokens":
            raise ProviderError("Claude's answer was cut off. Try a narrower question.")
        blocks = [b.model_dump(mode="json", exclude_none=True) for b in response.content]
        yield ChatReply(
            text="".join(b.text for b in response.content if b.type == "text"),
            tool_calls=[
                ToolCall(b.id, b.name, dict(b.input))
                for b in response.content
                if b.type == "tool_use"
            ],
            raw={"role": "assistant", "content": blocks},
            request=request,
            response=response.model_dump(mode="json"),
        )


def _is_openai_chat_model(model_id: str) -> bool:
    m = model_id.lower()
    if not (m.startswith(("gpt-", "chatgpt-")) or re.match(r"o\d", m)):
        return False
    excluded = ("audio", "realtime", "tts", "transcribe", "image", "search", "embedding",
                "moderation", "instruct")
    return not any(word in m for word in excluded)


def _is_gemini_text_model(model_id: str) -> bool:
    m = model_id.lower()
    return m.startswith("gemini") and not any(
        word in m for word in ("embedding", "image", "tts", "live", "audio")
    )


class OpenAICompatibleProvider(Provider):
    """OpenAI's chat completions API, which Google also serves for Gemini."""

    tasks = frozenset(TASKS)

    def __init__(self, id, label, base_url, model_filter, custom_base_url=False, key_hint=""):
        self.id = id
        self.label = label
        self.default_base_url = base_url
        self.model_filter = model_filter
        fields = [Field("api_key", "API key", secret=True, placeholder=key_hint)]
        if custom_base_url:
            fields.append(Field(
                "base_url", "Base URL", required=False, default=base_url,
                placeholder=f"{base_url} (or any OpenAI-compatible API)",
            ))
        self.fields = tuple(fields)

    def _base_url(self, cfg: dict) -> str:
        return (cfg.get("base_url") or self.default_base_url).rstrip("/")

    def _headers(self, cfg: dict) -> dict:
        return {"Authorization": f"Bearer {cfg['api_key']}"}

    def list_models(self, cfg: dict) -> list[dict]:
        base_url = self._base_url(cfg)
        body = _send(self.label, "GET", f"{base_url}/models", headers=self._headers(cfg))
        ids = [str(m.get("id", "")).removeprefix("models/") for m in body.get("data", [])]
        # Only filter the provider's own catalogue; other compatible APIs name models freely.
        if base_url == self.default_base_url.rstrip("/"):
            ids = [i for i in ids if self.model_filter(i)]
        return [{"id": i, "label": i} for i in sorted(set(filter(None, ids)))]

    def complete_json(self, cfg, model, system, prompt, schema, name) -> Call:
        request = {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": name, "strict": True, "schema": schema},
            },
        }
        body = _send(
            self.label, "POST", f"{self._base_url(cfg)}/chat/completions",
            headers=self._headers(cfg), body=request, timeout=300.0,
        )
        try:
            choice = body["choices"][0]
            message = choice["message"]
        except (KeyError, IndexError, TypeError):
            raise ProviderError(f"{self.label} returned an unexpected response.")
        if message.get("refusal"):
            raise ProviderError(f"{self.label} declined this request: {message['refusal']}")
        if choice.get("finish_reason") == "length":
            raise ProviderError(f"{self.label}'s answer was cut off. Try fewer transactions.")
        return Call(_parse_json(self.label, message.get("content")), request, body)

    def _messages(self, system: str, history: list[dict]) -> list[dict]:
        messages: list[dict] = [{"role": "system", "content": system}]
        for m in history:
            if m["role"] == "user":
                messages.append({"role": "user", "content": m["content"]})
            elif m["role"] == "assistant":
                if m.get("raw_provider") == self.id and m.get("raw"):
                    messages.append(m["raw"])
                    continue
                message = {"role": "assistant", "content": m.get("content") or None}
                if m.get("tool_calls"):
                    message["tool_calls"] = [
                        {
                            "id": c["id"],
                            "type": "function",
                            "function": {
                                "name": c["name"],
                                "arguments": json.dumps(c["arguments"]),
                            },
                        }
                        for c in m["tool_calls"]
                    ]
                messages.append(message)
            else:
                messages.append(
                    {"role": "tool", "tool_call_id": m["tool_call_id"], "content": m["content"]}
                )
        return messages

    def stream_chat_turn(self, cfg, model, system, history, tools, answer_only=False):
        request = {
            "model": model,
            "messages": self._messages(system, history),
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t["description"],
                        "parameters": t["input_schema"],
                    },
                }
                for t in tools
            ],
            "stream": True,
        }
        if answer_only:
            request["tool_choice"] = "none"
        text, refusal, finish = [], [], None
        calls: dict[int, dict] = {}
        chunks = []
        for line in _stream_lines(
            self.label, f"{self._base_url(cfg)}/chat/completions",
            headers=self._headers(cfg), body=request,
        ):
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            try:
                chunk = json.loads(data)
            except ValueError:
                raise ProviderError(f"{self.label} sent a malformed streaming response.")
            chunks.append(chunk)
            for choice in chunk.get("choices") or []:
                delta = choice.get("delta") or {}
                if delta.get("content"):
                    text.append(delta["content"])
                    yield delta["content"]
                if delta.get("refusal"):
                    refusal.append(delta["refusal"])
                for part in delta.get("tool_calls") or []:
                    call = calls.setdefault(
                        part.get("index", len(calls)),
                        {"id": None, "type": "function", "function": {"name": "", "arguments": ""}},
                    )
                    function = part.get("function") or {}
                    call["function"]["name"] += function.get("name") or ""
                    call["function"]["arguments"] += function.get("arguments") or ""
                    for key, value in part.items():
                        # Keep provider extras such as Gemini's thought signatures.
                        if key not in ("index", "function", "type") and value is not None:
                            call[key] = value
                finish = choice.get("finish_reason") or finish
        if refusal:
            raise ProviderError(f"{self.label} declined this request: {''.join(refusal)}")
        if finish == "length":
            raise ProviderError(f"{self.label}'s answer was cut off. Try a narrower question.")
        ordered = [calls[i] for i in sorted(calls)]
        for i, call in enumerate(ordered):
            call["id"] = call["id"] or f"call_{i}"
        raw = {"role": "assistant"}
        if text:
            raw["content"] = "".join(text)
        if ordered:
            raw["tool_calls"] = ordered
        yield ChatReply(
            text="".join(text),
            tool_calls=[
                ToolCall(
                    c["id"],
                    c["function"]["name"],
                    _parse_arguments(self.label, c["function"]["arguments"]),
                )
                for c in ordered
            ],
            raw=raw,
            request=request,
            response={"chunks": chunks},
        )


class OllamaProvider(Provider):
    id = "ollama"
    label = "Ollama (self-hosted)"
    fields = (Field("url", "Server URL", placeholder="http://ollama:11434"),)
    tasks = frozenset(TASKS)

    def _url(self, cfg: dict) -> str:
        return cfg["url"].rstrip("/")

    def list_models(self, cfg: dict) -> list[dict]:
        body = _send(self.label, "GET", f"{self._url(cfg)}/api/tags", timeout=30.0)
        names = sorted({m.get("name", "") for m in body.get("models", [])} - {""})
        return [{"id": n, "label": n} for n in names]

    def complete_json(self, cfg, model, system, prompt, schema, name) -> Call:
        request = {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                # Stops Qwen3-style models spending their budget on visible reasoning.
                {"role": "user", "content": f"/no_think\n{prompt}"},
            ],
            "format": schema,
            "stream": False,
        }
        body = _send(self.label, "POST", f"{self._url(cfg)}/api/chat", body=request, timeout=300.0)
        content = (body.get("message") or {}).get("content")
        return Call(_parse_json(self.label, content), request, body)

    @staticmethod
    def _messages(system: str, history: list[dict]) -> list[dict]:
        messages: list[dict] = [{"role": "system", "content": system}]
        for m in history:
            if m["role"] == "user":
                messages.append({"role": "user", "content": m["content"]})
            elif m["role"] == "assistant":
                if m.get("raw_provider") == "ollama" and m.get("raw"):
                    messages.append(m["raw"])
                    continue
                message = {"role": "assistant", "content": m.get("content") or ""}
                if m.get("tool_calls"):
                    message["tool_calls"] = [
                        {"function": {"name": c["name"], "arguments": c["arguments"]}}
                        for c in m["tool_calls"]
                    ]
                messages.append(message)
            else:
                messages.append({"role": "tool", "tool_name": m["name"], "content": m["content"]})
        return messages

    def stream_chat_turn(self, cfg, model, system, history, tools, answer_only=False):
        request = {
            "model": model,
            "messages": self._messages(system, history),
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t["description"],
                        "parameters": t["input_schema"],
                    },
                }
                for t in tools
            ],
            "stream": True,
        }
        # Ollama has no way to switch tools off for one request, so answer_only relies on
        # the prompt and the chat loop discarding any calls it makes anyway.
        text, tool_calls, chunks = [], [], []
        for line in _stream_lines(self.label, f"{self._url(cfg)}/api/chat", body=request):
            if not line.strip():
                continue
            try:
                chunk = json.loads(line)
            except ValueError:
                raise ProviderError(f"{self.label} sent a malformed streaming response.")
            if chunk.get("error"):
                raise ProviderError(f"{self.label} returned an error: {chunk['error']}")
            chunks.append(chunk)
            message = chunk.get("message") or {}
            if message.get("content"):
                text.append(message["content"])
                yield message["content"]
            tool_calls.extend(message.get("tool_calls") or [])
        raw = {"role": "assistant", "content": "".join(text)}
        if tool_calls:
            raw["tool_calls"] = tool_calls
        # Ollama doesn't give tool calls IDs; number them so results can be matched up.
        yield ChatReply(
            text="".join(text),
            tool_calls=[
                ToolCall(
                    f"call_{i}",
                    c["function"]["name"],
                    _parse_arguments(self.label, c["function"].get("arguments")),
                )
                for i, c in enumerate(tool_calls)
            ],
            raw=raw,
            request=request,
            response={"chunks": chunks},
        )


class TypeSafeProvider(Provider):
    id = "typesafe"
    label = "TypeSafe AI (Jev)"
    fields = (Field("api_key", "API key", secret=True),)
    tasks = frozenset(TASKS) - {"rules", "chat"}
    base_url = "https://api.typesafe.ai/v1"

    def _headers(self, cfg: dict) -> dict:
        return {"Authorization": f"Bearer {cfg['api_key']}"}

    def list_models(self, cfg: dict) -> list[dict]:
        body = _send(self.label, "GET", f"{self.base_url}/models", headers=self._headers(cfg))
        return [
            {"id": m["name"], "label": m["name"]}
            for m in body.get("models", [])
            if m.get("name")
        ]

    def choose(self, cfg: dict, model: str, state: dict, questions: dict) -> Call:
        """Ask Choice questions about `state`; returns the answers keyed like `questions`."""
        request = {"state": state, "model": model, "questions": questions}
        body = _send(
            self.label, "POST", f"{self.base_url}/systemone",
            headers=self._headers(cfg), body=request,
        )
        answers = body.get("answers")
        if not isinstance(answers, dict):
            raise ProviderError(f"{self.label} returned an unexpected response.")
        return Call(answers, request, body)


class VertexProvider(Provider):
    """Gemini on Google Cloud Vertex AI, with an express mode API key. Express mode keys only
    work against Vertex's global endpoint, in Google's own request format (not the
    OpenAI-compatible one the Gemini API provider uses)."""

    id = "vertex"
    label = "Google Vertex AI"
    fields = (Field("api_key", "Express mode API key", secret=True, placeholder="AQ...."),)
    tasks = frozenset(TASKS)
    base_url = "https://aiplatform.googleapis.com/v1/publishers/google/models"
    # Express mode has no model listing; these are Google's current Gemini models.
    MODELS = (
        ("gemini-3.8-flash", "Gemini 3.8 Flash"),
        ("gemini-3.5-flash", "Gemini 3.5 Flash"),
        ("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite"),
        ("gemini-3-pro-preview", "Gemini 3 Pro (preview)"),
        ("gemini-2.5-pro", "Gemini 2.5 Pro"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite"),
    )

    def _headers(self, cfg: dict) -> dict:
        return {"x-goog-api-key": cfg["api_key"]}

    def _url(self, model: str, method: str) -> str:
        return f"{self.base_url}/{model}:{method}"

    def list_models(self, cfg: dict) -> list[dict]:
        # Nothing to list, so check the key with a free token count instead.
        _send(
            self.label,
            "POST",
            self._url("gemini-2.5-flash", "countTokens"),
            headers=self._headers(cfg),
            body={"contents": [{"role": "user", "parts": [{"text": "hi"}]}]},
            timeout=30.0,
        )
        return [{"id": model_id, "label": label} for model_id, label in self.MODELS]

    def _check(self, body: dict) -> dict:
        blocked = (body.get("promptFeedback") or {}).get("blockReason")
        if blocked:
            raise ProviderError(f"{self.label} declined this request ({blocked}).")
        candidates = body.get("candidates") or []
        if not candidates:
            raise ProviderError(f"{self.label} returned no answer.")
        return candidates[0]

    @staticmethod
    def _finish(label: str, reason: str | None) -> None:
        if reason == "MAX_TOKENS":
            raise ProviderError(f"{label}'s answer was cut off. Try fewer transactions.")
        if reason in ("SAFETY", "RECITATION", "PROHIBITED_CONTENT", "BLOCKLIST", "SPII"):
            raise ProviderError(f"{label} declined this request ({reason}).")

    def complete_json(self, cfg, model, system, prompt, schema, name) -> Call:
        request = {
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": [{"role": "user", "parts": [{"text": prompt}]}],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": schema,
            },
        }
        body = _send(
            self.label,
            "POST",
            self._url(model, "generateContent"),
            headers=self._headers(cfg),
            body=request,
            timeout=300.0,
        )
        candidate = self._check(body)
        self._finish(self.label, candidate.get("finishReason"))
        parts = (candidate.get("content") or {}).get("parts") or []
        text = "".join(p.get("text", "") for p in parts if not p.get("thought"))
        return Call(_parse_json(self.label, text), {"model": model, **request}, body)

    def _contents(self, history: list[dict]) -> list[dict]:
        contents: list[dict] = []
        for m in history:
            if m["role"] == "user":
                contents.append({"role": "user", "parts": [{"text": m["content"]}]})
            elif m["role"] == "assistant":
                if m.get("raw_provider") == self.id and m.get("raw"):
                    # Replayed as returned, so Gemini's thought signatures survive
                    contents.append(m["raw"])
                    continue
                parts = [{"text": m["content"]}] if m.get("content") else []
                parts += [
                    {"functionCall": {"name": c["name"], "args": c["arguments"]}}
                    for c in m.get("tool_calls") or []
                ]
                if parts:
                    contents.append({"role": "model", "parts": parts})
            else:
                try:
                    result = json.loads(m["content"])
                except ValueError:
                    result = m["content"]
                response = result if isinstance(result, dict) else {"result": result}
                part = {"functionResponse": {"name": m["name"], "response": response}}
                # Results for one turn's calls go back together in a single user message
                if contents and contents[-1].get("_tool_results"):
                    contents[-1]["parts"].append(part)
                else:
                    contents.append({"role": "user", "parts": [part], "_tool_results": True})
        for content in contents:
            content.pop("_tool_results", None)
        return contents

    def stream_chat_turn(self, cfg, model, system, history, tools, answer_only=False):
        request = {
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": self._contents(history),
        }
        if tools:
            request["tools"] = [
                {
                    "functionDeclarations": [
                        {
                            "name": t["name"],
                            "description": t["description"],
                            "parametersJsonSchema": t["input_schema"],
                        }
                        for t in tools
                    ]
                }
            ]
            if answer_only:
                request["toolConfig"] = {"functionCallingConfig": {"mode": "NONE"}}
        parts: list[dict] = []
        chunks = []
        finish = None
        for line in _stream_lines(
            self.label,
            f"{self._url(model, 'streamGenerateContent')}?alt=sse",
            headers=self._headers(cfg),
            body=request,
        ):
            if not line.startswith("data:"):
                continue
            try:
                chunk = json.loads(line[5:].strip())
            except ValueError:
                raise ProviderError(f"{self.label} sent a malformed streaming response.")
            chunks.append(chunk)
            candidate = self._check(chunk)
            finish = candidate.get("finishReason") or finish
            for part in (candidate.get("content") or {}).get("parts") or []:
                parts.append(part)
                if part.get("text") and not part.get("thought"):
                    yield part["text"]
        self._finish(self.label, finish)
        text = "".join(p.get("text", "") for p in parts if p.get("text") and not p.get("thought"))
        calls = [
            ToolCall(
                p["functionCall"].get("id") or f"call_{i}",
                p["functionCall"]["name"],
                _parse_arguments(self.label, p["functionCall"].get("args") or {}),
            )
            for i, p in enumerate(p for p in parts if p.get("functionCall"))
        ]
        yield ChatReply(
            text=text,
            tool_calls=calls,
            raw={"role": "model", "parts": parts},
            request={"model": model, **request},
            response={"chunks": chunks},
        )


PROVIDERS: dict[str, Provider] = {
    p.id: p
    for p in (
        AnthropicProvider(),
        OpenAICompatibleProvider(
            "openai", "OpenAI", "https://api.openai.com/v1", _is_openai_chat_model,
            custom_base_url=True, key_hint="sk-...",
        ),
        OpenAICompatibleProvider(
            "google", "Google (Gemini)",
            "https://generativelanguage.googleapis.com/v1beta/openai", _is_gemini_text_model,
        ),
        VertexProvider(),
        TypeSafeProvider(),
        OllamaProvider(),
    )
}


def provider_config(db: Session, provider: Provider) -> dict:
    return {
        f.key: provider_value(db, provider.id, f.key) or f.default
        for f in provider.fields
    }


def missing_fields(provider: Provider, cfg: dict) -> list[str]:
    return [f.label for f in provider.fields if f.required and not cfg.get(f.key)]


def resolve_task(db: Session, task: str) -> tuple[Provider, dict, str]:
    """The provider, its settings and the model chosen for `task`, or a ProviderError
    telling the user what to set up."""
    provider_id, model = get_task(db, task)
    task_label = TASKS[task].lower()
    provider = PROVIDERS.get(provider_id or "")
    if not provider or not model:
        raise ProviderError(f"Choose an AI provider and model for {task_label} in Settings > AI.")
    if task not in provider.tasks:
        raise ProviderError(f"{provider.label} can't be used for {task_label}.")
    cfg = provider_config(db, provider)
    problem = get_provider_problem(db, provider.id)
    if problem:
        raise ProviderError(problem)
    missing = missing_fields(provider, cfg)
    if missing:
        raise ProviderError(
            f"{provider.label} isn't set up yet: add its {', '.join(missing)} "
            "in Settings > AI."
        )
    return provider, cfg, model
