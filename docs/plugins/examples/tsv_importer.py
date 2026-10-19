#!/usr/bin/env python3
"""A PennyChest importer plugin in Python, using only the standard library, to show the
protocol isn't tied to Rust. It reads tab-separated statements:

    2026-09-20<TAB>TESCO STORES 2041<TAB>-12.50

See docs/plugins/protocol.md.
"""

import base64
import json
import sys
from datetime import date
from decimal import Decimal, InvalidOperation

PROTOCOL_VERSION = 1
IMPORTER = {
    "name": "tsv",
    "label": "Tab-separated statements",
    "description": "Date, description and amount, separated by tabs",
    "file_types": ["tsv"],
    "detects": False,
    "statement_info": False,
}


class Failed(Exception):
    """A problem to show the user (error code -32000)."""


def parse(file: bytes) -> list[dict]:
    rows = []
    for number, line in enumerate(file.decode("utf-8-sig").splitlines(), start=1):
        if not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) != 3:
            raise Failed(f"Line {number}: expected a date, description and amount.")
        when, description, amount = fields
        try:
            date.fromisoformat(when)
            amount = str(Decimal(amount))
        except (ValueError, InvalidOperation):
            raise Failed(f"Line {number}: {line!r} isn't a date, description and amount.")
        rows.append(
            {
                "date": when,
                "description": description,
                "amount": amount,
                "raw_content": line,
                "line_number": number,
            }
        )
    return rows


def handle(method: str, params: dict):
    if method == "initialize":
        return {
            "protocol_version": PROTOCOL_VERSION,
            "name": "tsv",
            "version": "1.0.0",
            "importers": [IMPORTER],
            "exporters": [],
        }
    if method == "importer/parse":
        return {"rows": parse(base64.b64decode(params["file"]))}
    if method == "shutdown":
        return None
    raise LookupError(method)


def main() -> None:
    for line in sys.stdin:
        if not line.strip():
            continue
        request = json.loads(line)
        reply = {"jsonrpc": "2.0", "id": request["id"]}
        try:
            reply["result"] = handle(request["method"], request.get("params") or {})
        except Failed as e:
            reply["error"] = {"code": -32000, "message": str(e)}
        except LookupError:
            reply["error"] = {"code": -32601, "message": f"Method not found: {request['method']}"}
        print(json.dumps(reply), flush=True)
        if request["method"] == "shutdown":
            break


if __name__ == "__main__":
    main()
