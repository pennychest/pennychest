"""PennyChest's actions as an MCP server, over Streamable HTTP at /mcp.

Stateless and JSON-only: each POST carries one JSON-RPC message and gets one JSON reply, which
the transport allows. Agents authenticate with a personal access token from Settings, and can
only run the actions their token's scopes allow.
"""

import json
from importlib.metadata import version
from typing import Any

from fastapi import APIRouter, Depends, Request
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import JSONResponse, Response
from sqlalchemy.orm import Session

from pennychest.actions.catalog import ACTIONS, ActionError, describe_actions, run_action
from pennychest.actions.registry import SCOPE_LABELS, Scope
from pennychest.core.database import get_db
from pennychest.mcp.oauth import resource_metadata_url
from pennychest.mcp.tokens import find_token

SUPPORTED_VERSIONS = ("2025-11-25", "2025-06-18", "2025-03-26")
SERVER_INFO = {"name": "PennyChest", "version": version("pennychest")}
DESTRUCTIVE = {"delete_transactions", "delete_budget"}

router = APIRouter(tags=["mcp"])


def _instructions(scopes: list[str]) -> str:
    allowed = [SCOPE_LABELS[Scope(s)].lower() for s in scopes]
    can = (
        f"This token can also {' and '.join(allowed)}; every change is listed in PennyChest's "
        "Settings > Changes made by AI, where the user can undo it."
        if allowed
        else "This token is read-only."
    )
    return (
        "PennyChest is the user's personal finance ledger (double-entry, amounts in GBP unless "
        "stated). Use these tools to look up their accounts, spending, budgets and card taps. "
        + can
    )


def _tool(action: dict) -> dict:
    read_only = action["scope"] == Scope.READ
    return {
        "name": action["name"],
        "description": action["description"],
        "inputSchema": action["input_schema"],
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": action["name"] in DESTRUCTIVE,
            "openWorldHint": False,
        },
    }


def _result(id_: Any, result: dict) -> JSONResponse:
    return JSONResponse({"jsonrpc": "2.0", "id": id_, "result": result})


def _error(id_: Any, code: int, message: str, status: int = 200) -> JSONResponse:
    body = {"jsonrpc": "2.0", "id": id_, "error": {"code": code, "message": message}}
    return JSONResponse(body, status_code=status)


def unauthorised(metadata_url: str) -> Response:
    # Points OAuth connectors at the discovery document (RFC 9728) so they can sign in
    return JSONResponse(
        {"error": "invalid_token", "error_description": "Use a PennyChest access token."},
        status_code=401,
        headers={
            "WWW-Authenticate": f'Bearer realm="PennyChest", resource_metadata="{metadata_url}"'
        },
    )


@router.post("/mcp")
async def mcp_endpoint(request: Request, db: Session = Depends(get_db)) -> Response:
    body = await request.body()
    # Actions use the database synchronously, so they run off the event loop.
    return await run_in_threadpool(
        handle,
        db,
        request.headers.get("authorization", ""),
        body,
        resource_metadata_url(request),
    )


def handle(db: Session, authorization: str, body: bytes, metadata_url: str = "") -> Response:
    bearer = authorization[7:].strip() if authorization.lower().startswith("bearer ") else None
    token = find_token(db, bearer)
    if not token:
        return unauthorised(metadata_url)

    try:
        message = json.loads(body)
    except ValueError:
        return _error(None, -32700, "Parse error", status=400)
    if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
        return _error(None, -32600, "Expected a single JSON-RPC 2.0 message", status=400)

    method = message.get("method")
    id_ = message.get("id")
    params = message.get("params") or {}
    if method is None or "id" not in message:
        # Notifications and responses need no reply
        return Response(status_code=202)

    if method == "initialize":
        requested = params.get("protocolVersion")
        base = metadata_url.split("/.well-known/", 1)[0]
        return _result(
            id_,
            {
                "protocolVersion": requested
                if requested in SUPPORTED_VERSIONS
                else SUPPORTED_VERSIONS[0],
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {
                    **SERVER_INFO,
                    "title": "PennyChest",
                    "websiteUrl": base,
                    # Shown by clients that display server icons (MCP 2025-11-25)
                    "icons": [
                        {
                            "src": f"{base}/pwa-512x512.png",
                            "mimeType": "image/png",
                            "sizes": ["512x512"],
                        },
                        {
                            "src": f"{base}/pwa-192x192.png",
                            "mimeType": "image/png",
                            "sizes": ["192x192"],
                        },
                        {
                            "src": f"{base}/favicon.svg",
                            "mimeType": "image/svg+xml",
                            "sizes": ["any"],
                        },
                    ],
                },
                "instructions": _instructions(token.scopes),
            },
        )
    if method == "ping":
        return _result(id_, {})
    if method == "tools/list":
        return _result(id_, {"tools": [_tool(a) for a in describe_actions(token.scopes)]})
    if method == "tools/call":
        name = params.get("name")
        if name not in ACTIONS:
            return _error(id_, -32602, f"Unknown tool: {name}")
        try:
            output = run_action(
                db, name, params.get("arguments"), scopes=token.scopes, source="mcp"
            )
        except ActionError as e:
            return _result(id_, {"content": [{"type": "text", "text": str(e)}], "isError": True})
        return _result(
            id_,
            {
                "content": [{"type": "text", "text": json.dumps(output, default=str)}],
                "structuredContent": output,
                "isError": False,
            },
        )
    return _error(id_, -32601, f"Method not found: {method}")


@router.get("/mcp")
@router.delete("/mcp")
def mcp_no_stream() -> Response:
    # No server-initiated messages or sessions, so there's no stream to open or close.
    return Response(status_code=405, headers={"Allow": "POST"})
